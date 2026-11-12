use super::*;

/// RFC 4648 section 10.
const VECTORS: [(&str, &str); 7] = [
    ("", ""),
    ("f", "Zg=="),
    ("fo", "Zm8="),
    ("foo", "Zm9v"),
    ("foob", "Zm9vYg=="),
    ("fooba", "Zm9vYmE="),
    ("foobar", "Zm9vYmFy"),
];

#[test]
fn the_rfc_vectors_encode_and_decode() {
    for (plain, text) in VECTORS {
        assert_eq!(encode(plain.as_bytes()), text);
        assert_eq!(decode(text.as_bytes(), 16).unwrap(), plain.as_bytes());
        // Padding is optional on the way in.
        assert_eq!(
            decode(text.trim_end_matches('=').as_bytes(), 16).unwrap(),
            plain.as_bytes()
        );
    }
    assert_eq!(encode(&[0xfb, 0xff, 0xfe]), "+//+");
}

#[test]
fn every_length_and_byte_round_trips() {
    let bytes: Vec<u8> = (0..=255).collect();
    for len in 0..=bytes.len() {
        let text = encode(&bytes[..len]);
        assert_eq!(text.len(), len.div_ceil(3) * 4);
        assert_eq!(decode(text.as_bytes(), len).unwrap(), &bytes[..len]);
    }
}

#[test]
fn decoding_is_bounded_and_strict() {
    // The limit is on the decoded size, and is checked before the characters are looked at.
    assert_eq!(decode(b"Zm9vYmFy", 6).unwrap(), b"foobar");
    assert_eq!(
        decode(b"Zm9vYmFy", 5),
        Err(DecodeError::TooLarge { bytes: 6, cap: 5 })
    );
    assert_eq!(
        decode(b"@@@@@@@@", 5),
        Err(DecodeError::TooLarge { bytes: 6, cap: 5 })
    );
    assert_eq!(
        decode(b"Zg==", 0),
        Err(DecodeError::TooLarge { bytes: 1, cap: 0 })
    );
    let long = vec![b'A'; 4 * 1000];
    assert_eq!(
        decode(&long, 100),
        Err(DecodeError::TooLarge {
            bytes: 3000,
            cap: 100
        })
    );
    // A length no encoder writes.
    assert_eq!(decode(b"a", 10), Err(DecodeError::Length));
    assert_eq!(decode(b"Zm9vY", 10), Err(DecodeError::Length));
    // Characters outside the alphabet, padding inside, a third pad, whitespace, the URL-safe alphabet.
    for bad in [
        "aGV$bG8=",
        "Zg=A",
        "Zg===",
        "=Zg=",
        "Zm9v\n",
        "Zm9 v",
        "-_-_",
        "Zm9\u{e9}",
    ] {
        let err = decode(bad.as_bytes(), 64).unwrap_err();
        assert!(
            matches!(err, DecodeError::Character | DecodeError::Length),
            "{bad:?}: {err}"
        );
    }
    assert_eq!(decode(b"aGV$bG8=", 10), Err(DecodeError::Character));
    assert_eq!(decode(b"aGVsbG8=", 10).unwrap(), b"hello");
    assert_eq!(decode(b"aGVsbG8", 10).unwrap(), b"hello");
}

#[test]
fn errors_read_as_their_cause() {
    assert_eq!(DecodeError::Length.to_string(), "a base64 length");
    assert_eq!(DecodeError::Character.to_string(), "a base64 character");
    assert_eq!(
        DecodeError::TooLarge { bytes: 6, cap: 5 }.to_string(),
        "6 decoded bytes are above the limit of 5"
    );
}
