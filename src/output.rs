//! Writing the report without dying on a closed pipe.
//!
//! Rust ignores SIGPIPE, so `println!` panics when the reader has gone
//! (`deploy-diff … | head`). Until 0.1.2 deploy-diff reset the signal through
//! its own `extern "C"` declaration of `signal` — unsafe code for a hygiene
//! matter (audit 3 of the homeserver, B110). Now the whole report is written
//! at once and a broken pipe is an ordinary error: the reader has gone, the
//! verdict stands.

use std::io::{ErrorKind, Write};

/// Writes `text` to `out` and returns the exit code: `code` (the verdict)
/// when the text is written or the reader has gone. Any other write error is
/// an error (2) — except that a STOP (1) stays a STOP: a deploy must not go
/// through because its report could not be printed.
pub fn finish(out: &mut impl Write, text: &str, code: u8) -> u8 {
    match out.write_all(text.as_bytes()).and_then(|()| out.flush()) {
        Ok(()) => code,
        Err(e) if e.kind() == ErrorKind::BrokenPipe => code,
        Err(e) => {
            eprintln!("deploy-diff: writing the report: {e}");
            if code == 1 { 1 } else { 2 }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::finish;
    use std::io::{self, ErrorKind, Write};

    struct Failing(ErrorKind);

    impl Write for Failing {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(self.0.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_closed_pipe_keeps_the_verdict() {
        assert_eq!(finish(&mut Failing(ErrorKind::BrokenPipe), "x", 0), 0);
        assert_eq!(finish(&mut Failing(ErrorKind::BrokenPipe), "x", 1), 1);
    }

    #[test]
    fn another_write_error_is_an_error_but_a_stop_stays_a_stop() {
        assert_eq!(finish(&mut Failing(ErrorKind::Other), "x", 0), 2);
        assert_eq!(finish(&mut Failing(ErrorKind::Other), "x", 1), 1);
    }

    #[test]
    fn the_text_arrives_whole() {
        let mut v = Vec::new();
        assert_eq!(finish(&mut v, "a\nb\n", 1), 1);
        assert_eq!(v, b"a\nb\n");
    }
}
