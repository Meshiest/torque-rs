use super::*;
use crate::blocks::{BLOCK_HASH_LEN, content_hash};
use crate::message::{ClientMessage, decode};

fn data(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn started(len: usize) -> (UploadClient, BeginRequest, Vec<u8>) {
    let content = data(len);
    let mut client = UploadClient::new();
    let begin = client
        .begin("build", b"meta".to_vec(), Box::new(content.clone()))
        .unwrap();
    (client, begin, content)
}

fn accepted(client: &mut UploadClient, begin: &BeginRequest, id: u32) -> Vec<Vec<u8>> {
    client
        .accepted(
            begin.handle,
            Accept {
                id,
                has_list: false,
                have: 0,
            },
        )
        .unwrap()
}

#[test]
fn begin_hashes_the_content_and_accept_sends_the_block_list() {
    let (mut client, begin, content) = started(BLOCK_BYTES * 2 + 5);
    assert_eq!(begin.hash, content_hash(&content));
    assert_eq!(
        (begin.size, begin.kind.as_str(), begin.meta.as_slice()),
        (content.len() as u64, "build", &b"meta"[..])
    );
    assert_eq!(client.status(begin.handle), Some(&Status::Waiting));
    let messages = accepted(&mut client, &begin, 7);
    assert_eq!(messages.len(), 1);
    let Ok(ClientMessage::BlockList { id, hashes }) = decode(&messages[0]) else {
        panic!("not a block list");
    };
    assert_eq!((id, hashes.len()), (7, 3 * BLOCK_HASH_LEN));
}

#[test]
fn a_server_that_holds_the_list_is_not_sent_it_again() {
    let (mut client, begin, _) = started(1000);
    let messages = client
        .accepted(
            begin.handle,
            Accept {
                id: 1,
                has_list: true,
                have: 0,
            },
        )
        .unwrap();
    assert!(messages.is_empty());
}

#[test]
fn a_request_is_answered_with_that_runs_bytes_and_progress_counts() {
    let (mut client, begin, content) = started(BLOCK_BYTES * 5);
    accepted(&mut client, &begin, 7);
    let message = client.request(7, 1, 2).unwrap();
    let Ok(ClientMessage::Data { id, start, bytes }) = decode(&message) else {
        panic!("not data");
    };
    assert_eq!((id, start), (7, 1));
    assert_eq!(bytes, &content[BLOCK_BYTES..3 * BLOCK_BYTES]);
    assert_eq!(
        client.status(begin.handle),
        Some(&Status::Sending {
            sent: 2 * BLOCK_BYTES as u64,
            total: 5 * BLOCK_BYTES as u64
        })
    );
    client.ended(7, Ok(()));
    assert_eq!(client.status(begin.handle), Some(&Status::Done));
}

#[test]
fn a_resumed_upload_starts_its_progress_at_what_the_server_holds() {
    let (mut client, begin, _) = started(BLOCK_BYTES * 4);
    client
        .accepted(
            begin.handle,
            Accept {
                id: 3,
                has_list: true,
                have: 3,
            },
        )
        .unwrap();
    assert_eq!(
        client.status(begin.handle),
        Some(&Status::Sending {
            sent: 3 * BLOCK_BYTES as u64,
            total: 4 * BLOCK_BYTES as u64
        })
    );
}

#[test]
fn a_hostile_servers_requests_fail_the_upload() {
    for (start, count) in [(0, 0), (0, 5), (4, 1), (u32::MAX, 2), (3, 2)] {
        let (mut client, begin, _) = started(BLOCK_BYTES * 4);
        accepted(&mut client, &begin, 1);
        let e = client.request(1, start, count).unwrap_err();
        assert!(
            matches!(e, ClientError::Protocol(_)),
            "{start} {count}: {e:?}"
        );
        assert!(matches!(
            client.status(begin.handle),
            Some(Status::Failed(_))
        ));
        // Once failed, further requests are ignored, not answered.
        assert_eq!(client.request(1, 0, 1).unwrap_err(), ClientError::Unknown);
    }
}

#[test]
fn a_server_that_keeps_asking_for_the_same_blocks_is_stopped() {
    let (mut client, begin, _) = started(BLOCK_BYTES * 4);
    accepted(&mut client, &begin, 1);
    let mut sent = 0u64;
    let stopped = loop {
        match client.request(1, 0, 4) {
            Ok(message) => sent += message.len() as u64,
            Err(e) => break e,
        }
        assert!(sent < 16 * BLOCK_BYTES as u64 * 4, "never stopped");
    };
    assert!(matches!(stopped, ClientError::Protocol(_)));
}

#[test]
fn a_request_for_an_upload_that_is_not_going_is_unknown() {
    let (mut client, begin, _) = started(1000);
    assert_eq!(client.request(9, 0, 1).unwrap_err(), ClientError::Unknown);
    // Before the accept there is no id to ask about.
    assert_eq!(client.request(0, 0, 1).unwrap_err(), ClientError::Unknown);
    accepted(&mut client, &begin, 9);
    client.ended(9, Err("Not a build.".to_string()));
    assert_eq!(
        client.status(begin.handle),
        Some(&Status::Failed("Not a build.".to_string()))
    );
    assert_eq!(client.request(9, 0, 1).unwrap_err(), ClientError::Unknown);
}

#[test]
fn a_file_that_changed_since_it_was_hashed_is_not_sent() {
    struct Changing(Vec<u8>, bool);
    impl Source for Changing {
        fn len(&self) -> u64 {
            self.0.len() as u64
        }
        fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
            let from = offset as usize;
            buf.copy_from_slice(&self.0[from..from + buf.len()]);
            if self.1 {
                buf[0] ^= 1;
            }
            Ok(())
        }
    }
    let mut client = UploadClient::new();
    let content = data(1000);
    // The first reads (the hashing) are honest; flip the switch after.
    let begin = client
        .begin("build", Vec::new(), Box::new(Changing(content, false)))
        .unwrap();
    let upload = client.uploads.get_mut(&begin.handle).unwrap();
    upload.source = Box::new(Changing(data(1000), true));
    client
        .accepted(
            begin.handle,
            Accept {
                id: 1,
                has_list: true,
                have: 0,
            },
        )
        .unwrap();
    let e = client.request(1, 0, 1).unwrap_err();
    assert!(matches!(e, ClientError::Source(_)));
}

#[test]
fn a_refusal_and_a_lost_connection_fail_the_upload() {
    let (mut client, begin, _) = started(1000);
    client.refused(begin.handle, "No room.".to_string());
    assert_eq!(
        client.status(begin.handle),
        Some(&Status::Failed("No room.".to_string()))
    );
    let (mut client, begin, _) = started(1000);
    accepted(&mut client, &begin, 1);
    client.disconnected();
    assert!(matches!(
        client.status(begin.handle),
        Some(Status::Failed(_))
    ));
}

#[test]
fn what_cannot_be_begun_is_refused_locally() {
    let mut client = UploadClient::new();
    let err = |client: &mut UploadClient, kind: &str, meta: Vec<u8>, bytes: Vec<u8>| {
        client.begin(kind, meta, Box::new(bytes)).unwrap_err()
    };
    assert_eq!(
        err(&mut client, "build", vec![], vec![]),
        ClientError::BadSize
    );
    assert!(matches!(
        err(&mut client, "", vec![], vec![1]),
        ClientError::BadRequest(_)
    ));
    assert!(matches!(
        err(&mut client, &"x".repeat(41), vec![], vec![1]),
        ClientError::BadRequest(_)
    ));
    assert!(matches!(
        err(&mut client, "build", vec![0; MAX_META_BYTES + 1], vec![1]),
        ClientError::BadRequest(_)
    ));
}

#[test]
fn the_client_keeps_at_most_eight_uploads_and_forgets_finished_ones() {
    let mut client = UploadClient::new();
    let mut handles = Vec::new();
    for k in 0..MAX_CLIENT_UPLOADS {
        handles.push(
            client
                .begin("build", vec![], Box::new(data(10 + k)))
                .unwrap()
                .handle,
        );
    }
    assert_eq!(
        client
            .begin("build", vec![], Box::new(data(10 + MAX_CLIENT_UPLOADS)))
            .unwrap_err(),
        ClientError::TooMany
    );
    client.refused(handles[0], "no".to_string());
    assert!(client.begin("build", vec![], Box::new(data(10))).is_ok());
}

#[test]
fn a_file_is_read_as_it_is_asked_for() {
    let exe = std::env::current_exe().unwrap();
    let path = exe
        .parent()
        .unwrap()
        .join(format!("source-{}.bin", std::process::id()));
    std::fs::write(&path, data(3000)).unwrap();
    let mut source = FileSource::open(&path).unwrap();
    assert_eq!(Source::len(&source), 3000);
    let mut buf = [0u8; 10];
    source.read_at(100, &mut buf).unwrap();
    assert_eq!(&buf, &data(3000)[100..110]);
    assert!(source.read_at(2995, &mut buf).is_err());
    let _ = std::fs::remove_file(&path);
}

#[test]
fn a_cancelled_upload_fails_and_later_requests_for_it_are_ignored() {
    let (mut client, begin, _) = started(1000);
    assert_eq!(
        client.cancel(begin.handle),
        None,
        "nothing accepted yet, nothing to tell the server"
    );
    assert_eq!(
        client.status(begin.handle),
        Some(&Status::Failed("Cancelled.".to_string()))
    );
    let (mut client, begin, _) = started(1000);
    accepted(&mut client, &begin, 5);
    assert_eq!(client.cancel(begin.handle), Some(5));
    assert_eq!(client.request(5, 0, 1).unwrap_err(), ClientError::Unknown);
    assert_eq!(client.cancel(begin.handle), None);
}

#[test]
fn the_same_content_is_not_begun_twice_while_the_first_is_going() {
    let mut client = UploadClient::new();
    let first = client
        .begin("build", b"a".to_vec(), Box::new(data(500)))
        .unwrap();
    assert_eq!(
        client
            .begin("build", b"b".to_vec(), Box::new(data(500)))
            .unwrap_err(),
        ClientError::Duplicate
    );
    // Another kind, other content, or the first one over: allowed.
    assert!(client.begin("other", vec![], Box::new(data(500))).is_ok());
    assert!(client.begin("build", vec![], Box::new(data(501))).is_ok());
    client.refused(first.handle, "No.".to_string());
    assert!(client.begin("build", vec![], Box::new(data(500))).is_ok());
}
