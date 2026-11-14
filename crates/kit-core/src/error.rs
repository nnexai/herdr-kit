/// The CLI error envelope, with control characters removed and the message
/// limited to 512 Unicode scalar values. Callers retain their own exit policy.
pub fn json(error: &anyhow::Error, code: &str) -> serde_json::Value {
    let message: String = error
        .to_string()
        .chars()
        .filter(|c| !c.is_control())
        .take(512)
        .collect();
    serde_json::json!({"error":{"code":code,"message":message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_bytes_preserve_code_escaping_and_unicode_limit() {
        let error = anyhow::anyhow!("\n\t\u{7f}{}\rnot included", "é\"".repeat(256));
        assert_eq!(
            json(&error, "not_ready").to_string(),
            format!(
                "{{\"error\":{{\"code\":\"not_ready\",\"message\":\"{}\"}}}}",
                "é\\\"".repeat(256)
            )
        );
    }
}
