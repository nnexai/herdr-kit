use std::fmt;

#[derive(Debug)]
pub struct CoordError {
    pub code: &'static str,
    pub message: String,
    pub exit: i32,
}

impl fmt::Display for CoordError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CoordError {}

pub fn refused(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    CoordError {
        code,
        message: message.into(),
        exit: 3,
    }
    .into()
}
pub fn not_ready(code: &'static str, message: impl Into<String>) -> anyhow::Error {
    CoordError {
        code,
        message: message.into(),
        exit: 4,
    }
    .into()
}
pub fn exit_of(e: &anyhow::Error) -> (&'static str, i32) {
    e.downcast_ref::<CoordError>()
        .map(|e| (e.code, e.exit))
        .unwrap_or(("operation_failed", 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exit_codes_survive_context() {
        let e = refused("forbidden_caller", "no").context("command");
        assert_eq!(exit_of(&e), ("forbidden_caller", 3));
        assert_eq!(
            exit_of(&not_ready("agent_not_ready", "wait")),
            ("agent_not_ready", 4)
        );
        assert_eq!(
            exit_of(&anyhow::anyhow!("failure")),
            ("operation_failed", 1)
        );
    }
}
