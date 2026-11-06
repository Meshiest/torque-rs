use super::*;

fn args(n: usize) -> Vec<String> {
    (0..n).map(|i| i.to_string()).collect()
}

/// A request run at once, its answer given back.
fn answer(r: Request) -> Result<String, BridgeError> {
    let out = Rc::new(RefCell::new(None));
    let slot = out.clone();
    request(r, Box::new(move |a| *slot.borrow_mut() = Some(a)));
    let got = out.borrow_mut().take();
    got.unwrap_or_else(|| Ok("<waiting>".into()))
}

fn vm() -> Vm {
    let mut vm = Vm::new();
    install(&mut vm);
    vm.eval(b"function add(%a, %b) { return %a + %b; } function echo(%a) { return %a; }")
        .unwrap();
    vm
}

#[test]
fn names_are_identifiers_within_the_cap() {
    assert!(is_name("onMapLoaded"));
    assert!(is_name("_x1"));
    assert!(!is_name(""));
    assert!(!is_name("1x"));
    assert!(!is_name("a::b"));
    assert!(!is_name("a b"));
    assert!(!is_name(&"x".repeat(MAX_NAME_BYTES + 1)));
    assert!(is_name(&"x".repeat(MAX_NAME_BYTES)));
    assert_eq!(
        var_name("pref::Video::quality").unwrap(),
        "$pref::Video::quality"
    );
    assert_eq!(var_name("$x").unwrap(), "$x");
    assert!(var_name("$").is_err());
    assert!(var_name("$a::").is_err());
    assert!(var_name("$a;b").is_err());
}

#[test]
fn arguments_are_capped_in_count_and_size() {
    assert_eq!(check_args(&args(MAX_ARGS)), Ok(()));
    assert_eq!(
        check_args(&args(MAX_ARGS + 1)),
        Err(BridgeError::TooManyArgs(MAX_ARGS + 1))
    );
    let long = vec!["a".into(), "x".repeat(MAX_ARG_BYTES + 1)];
    assert_eq!(
        check_args(&long),
        Err(BridgeError::TooLong {
            what: "argument 2".into(),
            bytes: MAX_ARG_BYTES + 1
        })
    );
    assert_eq!(check_args(&["x".repeat(MAX_ARG_BYTES)]), Ok(()));
    let text = "x".repeat(MAX_TEXT_BYTES + 1);
    assert_eq!(
        answer(Request::Eval { text }).unwrap_err().kind(),
        "too-long"
    );
    let value = "x".repeat(MAX_ARG_BYTES + 1);
    let set = Request::SetVar {
        name: "$a".into(),
        value,
    };
    assert_eq!(answer(set).unwrap_err().kind(), "too-long");
    let call = Request::Call {
        function: "add".into(),
        args: args(MAX_ARGS + 1),
    };
    assert_eq!(answer(call).unwrap_err().kind(), "too-many-args");
    assert_eq!(queued(), 0, "a refused request does not wait");
}

#[test]
fn requests_wait_for_the_frame_and_are_served_in_order() {
    let mut vm = vm();
    let seen = Rc::new(RefCell::new(Vec::new()));
    for r in [
        Request::SetVar {
            name: "$Page::Hello".into(),
            value: "world".into(),
        },
        Request::GetVar {
            name: "Page::Hello".into(),
        },
        Request::Call {
            function: "add".into(),
            args: vec!["2".into(), "3".into()],
        },
        Request::Eval {
            text: "return echo(\"hi\") @ \"!\";".into(),
        },
        Request::Call {
            function: "nothing".into(),
            args: vec![],
        },
    ] {
        let seen = seen.clone();
        request(r, Box::new(move |a| seen.borrow_mut().push(a)));
    }
    assert!(seen.borrow().is_empty(), "nothing runs before the frame");
    assert_eq!(queued(), 5);
    serve(&mut vm);
    assert_eq!(
        *seen.borrow(),
        vec![
            Ok(String::new()),
            Ok("world".into()),
            Ok("5".into()),
            Ok("hi!".into()),
            Err(BridgeError::NoFunction("nothing".into())),
        ]
    );
}

#[test]
fn a_frame_serves_at_most_its_share_and_the_queue_is_capped() {
    let mut vm = vm();
    let done = Rc::new(Cell::new(0));
    let refused = Rc::new(Cell::new(0));
    for _ in 0..MAX_QUEUED + 3 {
        let (done, refused) = (done.clone(), refused.clone());
        let r = Request::GetVar { name: "$x".into() };
        request(
            r,
            Box::new(move |a| match a {
                Ok(_) => done.set(done.get() + 1),
                Err(BridgeError::QueueFull) => refused.set(refused.get() + 1),
                Err(e) => panic!("{e}"),
            }),
        );
    }
    assert_eq!(refused.get(), 3);
    assert_eq!(queued(), MAX_QUEUED);
    serve(&mut vm);
    assert_eq!(done.get(), MAX_SERVED_PER_FRAME);
    assert_eq!(queued(), MAX_QUEUED - MAX_SERVED_PER_FRAME);
}

#[test]
fn a_syntax_error_runs_nothing_and_says_where() {
    let mut vm = vm();
    let first = Request::Eval {
        text: "$Ran = 1;\n$Also = 2 +;\n".into(),
    };
    let e = run(&mut vm, &first).unwrap_err();
    let BridgeError::Syntax { line, column } = e.clone() else {
        panic!("{e:?}");
    };
    assert_eq!(line, 2, "{e:?}");
    assert!(column.is_some_and(|c| (10..=12).contains(&c)), "{e:?}");
    assert_eq!(vm.global("$Ran"), None, "nothing of the text ran");
    let json: serde_json::Value = serde_json::from_str(&e.to_json()).unwrap();
    assert_eq!(json["kind"], "syntax");
    assert_eq!(json["line"], 2);
    let one = run(
        &mut vm,
        &Request::Eval {
            text: "echo(;".into(),
        },
    )
    .unwrap_err();
    assert!(
        matches!(
            one,
            BridgeError::Syntax {
                line: 1,
                column: Some(_)
            }
        ),
        "{one:?}"
    );
}

#[test]
fn script_reaches_only_the_handlers_the_page_registered() {
    let mut vm = vm();
    let calls = Rc::new(RefCell::new(Vec::new()));
    let seen = calls.clone();
    set_page(Some(Rc::new(move |name: &str, args: &[String]| {
        seen.borrow_mut().push((name.to_string(), args.to_vec()));
        Ok(format!("{name}:{}", args.join(",")))
    })));
    assert_eq!(
        call_page("onDied", &[]),
        Err(BridgeError::NotRegistered("onDied".into()))
    );
    register_handler("onDied").unwrap();
    assert!(register_handler("not a name").is_err());
    assert_eq!(
        vm.eval(b"return callPage(\"onDied\", \"Ann\", 3);")
            .unwrap(),
        b"onDied:Ann,3"
    );
    assert_eq!(
        vm.eval(b"return callPage(\"onWon\", \"Ann\");").unwrap(),
        b"",
        "an unregistered name reaches nothing"
    );
    unregister_handler("onDied");
    assert_eq!(vm.eval(b"return callPage(\"onDied\");").unwrap(), b"");
    assert_eq!(calls.borrow().len(), 1);
    set_page(None);
}

#[test]
fn script_calls_the_page_at_most_its_share_a_frame() {
    let mut vm = vm();
    set_page(Some(Rc::new(|_: &str, _: &[String]| Ok("1".into()))));
    register_handler("tick").unwrap();
    for _ in 0..MAX_PAGE_CALLS_PER_FRAME {
        assert_eq!(call_page("tick", &[]), Ok("1".into()));
    }
    assert_eq!(call_page("tick", &[]), Err(BridgeError::TooManyCalls));
    serve(&mut vm);
    assert_eq!(call_page("tick", &[]), Ok("1".into()), "a new frame");
    assert_eq!(
        call_page("tick", &args(MAX_ARGS + 1)),
        Err(BridgeError::TooManyArgs(MAX_ARGS + 1))
    );
    set_page(None);
}

#[test]
fn a_handler_may_make_requests_while_script_calls_it() {
    let mut vm = vm();
    set_page(Some(Rc::new(|_: &str, _: &[String]| {
        request(
            Request::SetVar {
                name: "$FromPage".into(),
                value: "yes".into(),
            },
            Box::new(|_| {}),
        );
        Ok(String::new())
    })));
    register_handler("loaded").unwrap();
    vm.eval(b"callPage(\"loaded\");").unwrap();
    assert_eq!(queued(), 1);
    serve(&mut vm);
    assert_eq!(vm.global("$FromPage").as_deref(), Some(&b"yes"[..]));
    set_page(None);
}

#[test]
fn no_page_no_call() {
    register_handler("x").unwrap();
    assert_eq!(call_page("x", &[]), Err(BridgeError::NoPage));
}

#[test]
fn script_sets_the_render_quality() {
    let mut vm = vm();
    assert_eq!(vm.eval(b"return setRenderQuality(\"low\");").unwrap(), b"1");
    assert_eq!(
        crate::web_page::take_quality(),
        Some(torque_render_scene::hd::Quality::Low)
    );
    assert_eq!(vm.eval(b"return setRenderQuality(\"max\");").unwrap(), b"0");
    assert_eq!(crate::web_page::take_quality(), None);
}
