use super::*;
use crate::vm::MAX_LOG_LINE;

#[test]
fn echo_joins_at_most_one_log_line() {
    let mut argv = vec![b"echo".to_vec()];
    argv.extend((0..64).map(|_| vec![b'a'; 1 << 20]));
    let line = joined(&argv);
    assert_eq!(line.len(), MAX_LOG_LINE);
    assert!(
        line.capacity() <= 2 * MAX_LOG_LINE,
        "nothing past the line is copied"
    );
    // Short arguments join whole, each ending at its NUL.
    let argv = [b"echo".to_vec(), b"ab\0c".to_vec(), b"d".to_vec()];
    assert_eq!(joined(&argv), b"abd");
}
