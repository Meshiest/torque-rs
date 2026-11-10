//! Connections: what an entity's output sends where, as the map stores them.
//!
//! **Entity text.** An output is a keyvalue whose key is the output's name and whose value is five fields: target, input,
//! parameter override (may be empty), delay in seconds, times to fire (`-1` forever). An output with several connections
//! repeats its key, so the pairs are read in order with duplicates kept. The separator is the ESC byte (`0x1B`) in later maps
//! and a comma in older ones: a value that holds an ESC is split on it, any other on commas. Either way the first two fields
//! are the target and input and the last two the delay and times, and whatever lies between is the parameter (so a parameter
//! may itself hold commas).
//!
//! **Which keys are outputs.** A value with an ESC is a connection under any key. A comma value is one only under a key that
//! starts with `On` or `Out` (any case; the names outputs have), so an ordinary keyvalue that happens to hold four commas is
//! not read as one. An FGD that lists the class's outputs widens this to its names ([`is_output_key`]).
//!
//! **The run-time form** (`AddOutput`, [`parse_add_output`]): `OnFoo target:input:parameter:delay:times`, colons for
//! separators, one connection per call.
//!
//! **Numbers.** The delay reads leniently ([`crate::value::atof`]); a negative or non-finite delay is 0 and the most is
//! [`MAX_DELAY`] seconds. Times to fire: a positive count, or forever for `-1`, `0` and any other number below 1 (our choice:
//! a count that is not positive never runs out). Each field is cut at [`MAX_FIELD`] bytes.

use crate::value::{atof, atoi};

/// The longest delay kept, in seconds (a day): a later one is cut to it.
pub const MAX_DELAY: f32 = 86_400.0;
/// The most bytes kept of one field.
pub const MAX_FIELD: usize = 1024;

/// One connection of an output.
#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub target: String,
    pub input: String,
    /// Replaces the output's own value when not empty.
    pub parameter: String,
    /// Seconds, 0 to [`MAX_DELAY`].
    pub delay: f32,
    /// How many more times it fires; None forever.
    pub times: Option<u32>,
}

impl Connection {
    /// As the format-neutral map layer carries it.
    pub fn to_output(&self, output: &str) -> torque_mapcore::scene::Output {
        torque_mapcore::scene::Output {
            output: output.to_string(),
            target: self.target.clone(),
            input: self.input.clone(),
            parameter: self.parameter.clone(),
            delay: self.delay,
            times: self.times,
        }
    }

    /// From the format-neutral map layer's record (its numbers held to this module's rules).
    pub fn from_output(o: &torque_mapcore::scene::Output) -> Connection {
        Connection {
            target: cut(o.target.trim()),
            input: cut(o.input.trim()),
            parameter: cut(&o.parameter),
            delay: delay_of(f64::from(o.delay)),
            times: o.times.filter(|&t| t > 0),
        }
    }
}

fn cut(s: &str) -> String {
    if s.len() <= MAX_FIELD {
        return s.to_string();
    }
    let mut end = MAX_FIELD;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

fn delay_of(d: f64) -> f32 {
    if d.is_finite() && d > 0.0 {
        d.min(f64::from(MAX_DELAY)) as f32
    } else {
        0.0
    }
}

fn times_of(s: &str) -> Option<u32> {
    let t = atoi(s);
    (t >= 1).then(|| t.min(i64::from(u32::MAX)) as u32)
}

/// Whether the delay and times fields read as numbers (a leading number each; `-1` for forever).
fn numeric(s: &str) -> bool {
    let t = s.trim();
    let t = t.strip_prefix(['-', '+']).unwrap_or(t);
    t.bytes()
        .next()
        .is_some_and(|c| c.is_ascii_digit() || c == b'.')
}

fn from_fields(fields: &[&str], sep: &str) -> Option<Connection> {
    if fields.len() < 5 {
        return None;
    }
    let n = fields.len();
    let (delay, times) = (fields[n - 2], fields[n - 1]);
    if !numeric(delay) || !numeric(times) {
        return None;
    }
    Some(Connection {
        target: cut(fields[0].trim()),
        input: cut(fields[1].trim()),
        parameter: cut(&fields[2..n - 2].join(sep)),
        delay: delay_of(atof(delay)),
        times: times_of(times),
    })
}

/// A connection from an output's value in the entity text (module docs); None when the value is not one.
pub fn parse_connection(value: &str) -> Option<Connection> {
    if value.contains('\u{1b}') {
        let fields: Vec<&str> = value.split('\u{1b}').take(64).collect();
        return from_fields(&fields, "\u{1b}");
    }
    let fields: Vec<&str> = value.split(',').take(64).collect();
    from_fields(&fields, ",")
}

/// Whether a key names an output by its spelling (`On...` or `Out...`, any case).
pub fn output_like(key: &str) -> bool {
    let k = key.as_bytes();
    k.len() > 2
        && (k[..2].eq_ignore_ascii_case(b"on") || k[..3.min(k.len())].eq_ignore_ascii_case(b"out"))
}

/// Whether the pair `key` / `value` is a connection (module docs). `known` is the class's output names from an FGD, when one
/// was read.
pub fn is_output_key(key: &str, value: &str, known: Option<&dyn Fn(&str) -> bool>) -> bool {
    if value.contains('\u{1b}') {
        return true;
    }
    (output_like(key) || known.is_some_and(|k| k(key))) && value.split(',').count() >= 5
}

/// `AddOutput`'s connection form: `OnFoo target:input:parameter:delay:times` (the times may be left out: forever). The output's
/// name and its connection, or None when the text is not that form.
pub fn parse_add_output(text: &str) -> Option<(String, Connection)> {
    let text = text.trim();
    let (name, rest) = text.split_once(char::is_whitespace)?;
    let rest = rest.trim();
    let fields: Vec<&str> = rest.split(':').take(64).collect();
    if fields.len() < 4 || name.is_empty() {
        return None;
    }
    let mut fields = fields;
    if fields.len() == 4 {
        fields.push("-1");
    }
    let c = from_fields(&fields, ":")?;
    Some((cut(name), c))
}

/// An output's name and one of its connections.
pub type NamedConnection = (String, Connection);

/// A map entity's keyvalues split into its plain keys and its outputs, both in the file's order.
pub fn split_outputs(
    pairs: &[(String, String)],
    known: Option<&dyn Fn(&str) -> bool>,
) -> (Vec<(String, String)>, Vec<NamedConnection>) {
    let mut keys = Vec::new();
    let mut outputs = Vec::new();
    for (k, v) in pairs {
        if is_output_key(k, v, known)
            && let Some(c) = parse_connection(v)
        {
            outputs.push((cut(k), c));
            continue;
        }
        keys.push((k.clone(), v.clone()));
    }
    (keys, outputs)
}

/// A map entity's keyvalues (the entity text's pairs, in order) as the format-neutral map layer's entity: its class, plain keys,
/// origin and outputs; a brush model `*N` as its mover.
pub fn entity_from_pairs(pairs: &[(String, String)]) -> torque_mapcore::scene::Entity {
    let (keys, outputs) = split_outputs(pairs, None);
    let get = |name: &str| {
        keys.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let class = get("classname").unwrap_or("").to_string();
    let origin = get("origin")
        .and_then(crate::value::vec3)
        .unwrap_or([0.0; 3]);
    let mover = get("model")
        .and_then(|m| m.strip_prefix('*'))
        .and_then(|n| n.parse::<u32>().ok())
        .filter(|&n| n > 0)
        .map(torque_mapcore::scene::Mover::BrushModel);
    torque_mapcore::scene::Entity {
        class,
        origin,
        mover,
        outputs: outputs.iter().map(|(o, c)| c.to_output(o)).collect(),
        keys,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
        list.iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn esc_and_comma_values_read_alike() {
        let a = parse_connection("door\u{1b}Open\u{1b}\u{1b}0.5\u{1b}-1").unwrap();
        let b = parse_connection("door,Open,,0.5,-1").unwrap();
        assert_eq!(a, b);
        assert_eq!(a.target, "door");
        assert_eq!(a.input, "Open");
        assert_eq!(a.parameter, "");
        assert_eq!(a.delay, 0.5);
        assert_eq!(a.times, None);
    }

    #[test]
    fn extra_separators_belong_to_the_parameter() {
        let c = parse_connection("t,AddOutput,a,b,c,1,2").unwrap();
        assert_eq!(c.parameter, "a,b,c");
        assert_eq!(c.delay, 1.0);
        assert_eq!(c.times, Some(2));
        let e = parse_connection("t\u{1b}In\u{1b}x,y\u{1b}0\u{1b}1").unwrap();
        assert_eq!(e.parameter, "x,y");
        assert_eq!(e.times, Some(1));
    }

    #[test]
    fn numbers_are_held_to_their_ranges() {
        let c = parse_connection("t,i,,-3,0").unwrap();
        assert_eq!((c.delay, c.times), (0.0, None));
        let c = parse_connection("t,i,,1e30,99999999999").unwrap();
        assert_eq!((c.delay, c.times), (MAX_DELAY, Some(u32::MAX)));
        assert!(parse_connection("t,i,,x,1").is_none());
        assert!(parse_connection("t,i,,1").is_none());
        assert!(parse_connection("").is_none());
    }

    #[test]
    fn only_output_like_keys_take_comma_values() {
        let p = pairs(&[
            ("classname", "logic_relay"),
            ("targetname", "r"),
            ("OnTrigger", "a,Open,,0,-1"),
            ("OnTrigger", "b,Close,,1,1"),
            ("message", "x,y,z,1,2"),
            ("Anything", "c\u{1b}Kill\u{1b}\u{1b}0\u{1b}-1"),
        ]);
        let (keys, outs) = split_outputs(&p, None);
        assert_eq!(keys.len(), 3);
        assert_eq!(outs.len(), 3);
        assert_eq!(outs[0].1.target, "a");
        assert_eq!(outs[1].1.target, "b");
        assert_eq!(outs[2].0, "Anything");
        let known = |k: &str| k == "message";
        let (_, outs) = split_outputs(&p, Some(&known));
        assert_eq!(outs.len(), 4);
    }

    #[test]
    fn add_output_takes_colons() {
        let (name, c) = parse_add_output("OnTrigger door:Open::2:1").unwrap();
        assert_eq!(name, "OnTrigger");
        assert_eq!((c.target.as_str(), c.input.as_str()), ("door", "Open"));
        assert_eq!((c.delay, c.times), (2.0, Some(1)));
        let (_, c) = parse_add_output("OnUser1 !self:FireUser2:p:0").unwrap();
        assert_eq!((c.parameter.as_str(), c.times), ("p", None));
        assert!(parse_add_output("targetname newname").is_none());
        assert!(parse_add_output("OnUser1").is_none());
    }

    #[test]
    fn an_entity_becomes_the_neutral_record() {
        let e = entity_from_pairs(&pairs(&[
            ("classname", "func_door"),
            ("model", "*3"),
            ("origin", "1 2 3"),
            ("OnFullyOpen", "x,Trigger,,0,-1"),
        ]));
        assert_eq!(e.class, "func_door");
        assert_eq!(e.origin, [1.0, 2.0, 3.0]);
        assert_eq!(e.mover, Some(torque_mapcore::scene::Mover::BrushModel(3)));
        assert_eq!(e.outputs.len(), 1);
        assert_eq!(e.outputs[0].output, "OnFullyOpen");
        let back = Connection::from_output(&e.outputs[0]);
        assert_eq!(back.input, "Trigger");
    }
}
