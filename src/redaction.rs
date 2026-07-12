use regex::{Regex, RegexBuilder};
use std::sync::OnceLock;

const REDACTED: &str = "[REDACTED_SECRET]";

struct RedactionPattern {
    pattern: Regex,
    _label: &'static str,
}

fn get_patterns() -> &'static Vec<RedactionPattern> {
    static PATTERNS: OnceLock<Vec<RedactionPattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        let raw_patterns = vec![
            // Database connection strings
            (r#"postgres(?:ql)?://[^\s"',)]+"#, "DB_URL", false),
            (r#"mysql2?://[^\s"',)]+"#, "MYSQL_URL", false),
            (r#"mongodb(?:\+srv)?://[^\s"',)]+"#, "MONGO_URL", false),
            (r#"redis://[^\s"',)]+"#, "REDIS_URL", false),

            // GitHub tokens
            (r"ghp_[A-Za-z0-9_]{36,}", "GITHUB_PAT", false),
            (r"github_pat_[A-Za-z0-9_]{82,}", "GITHUB_PAT_FINE", false),
            (r"ghs_[A-Za-z0-9_]{36,}", "GITHUB_SERVER_TOKEN", false),
            (r"gho_[A-Za-z0-9_]{36,}", "GITHUB_OAUTH_TOKEN", false),
            (r"ghu_[A-Za-z0-9_]{36,}", "GITHUB_USER_TOKEN", false),

            // Slack tokens
            (r"xox[baprs]-[A-Za-z0-9-]{10,}", "SLACK_TOKEN", false),

            // Stripe
            (r"sk_live_[A-Za-z0-9]{24,}", "STRIPE_LIVE_KEY", false),
            (r"sk_test_[A-Za-z0-9]{24,}", "STRIPE_TEST_KEY", false),
            (r"rk_live_[A-Za-z0-9]{24,}", "STRIPE_RESTRICTED_KEY", false),

            // Google / Firebase
            (r"AIza[A-Za-z0-9\-_]{30,}", "GOOGLE_API_KEY", false),
            (r"X-Goog-Api-Key:\s*[^\s\n]+", "GOOGLE_API_KEY_HEADER", true),

            // AWS
            (r"AKIA[A-Z0-9]{16}", "AWS_ACCESS_KEY", false),
            (r"(?:aws_secret_access_key|AWS_SECRET_ACCESS_KEY)\s*[=:]\s*\S+", "AWS_SECRET", true),

            // Anthropic
            (r"sk-ant-[A-Za-z0-9\-_]{40,}", "ANTHROPIC_KEY", false),

            // OpenAI
            (r"sk-[A-Za-z0-9]{32,}", "OPENAI_KEY", false),

            // Named env vars containing secrets
            (r"VERCEL_TOKEN\s*[=:]\s*\S+", "VERCEL_TOKEN", true),
            (r"SUPABASE_SERVICE_ROLE_KEY\s*[=:]\s*\S+", "SUPABASE_SERVICE_ROLE_KEY", true),
            (r"SUPABASE_ANON_KEY\s*[=:]\s*\S+", "SUPABASE_ANON_KEY", true),
            (r"CLERK_SECRET_KEY\s*[=:]\s*\S+", "CLERK_SECRET_KEY", true),
            (r"DATABASE_URL\s*[=:]\s*\S+", "DATABASE_URL", true),
            (r"DIRECT_URL\s*[=:]\s*\S+", "DIRECT_URL", true),
            (r#"(?i)(?:password|passwd|secret|token|api_key|apikey)\s*[=:]\s*["']?[A-Za-z0-9+/=\-_]{12,}["']?"#, "GENERIC_SECRET", true),

            // Bearer tokens in headers
            (r"Authorization:\s*Bearer\s+[A-Za-z0-9\-._~+/]+=*", "BEARER_TOKEN", true),

            // Private keys
            (r"(?s)-----BEGIN (?:RSA |EC |OPENSSH )?PRIVATE KEY-----.+?-----END (?:RSA |EC |OPENSSH )?PRIVATE KEY-----", "PRIVATE_KEY", false),
        ];

        raw_patterns
            .into_iter()
            .map(|(pat, label, case_insensitive)| {
                let mut builder = RegexBuilder::new(pat);
                if case_insensitive {
                    builder.case_insensitive(true);
                }
                RedactionPattern {
                    pattern: builder.build().unwrap(),
                    _label: label,
                }
            })
            .collect()
    })
}

pub struct RedactResult {
    pub output: String,
    pub triggered: bool,
}

pub fn redact(text: &str) -> RedactResult {
    let mut output = text.to_string();
    let mut triggered = false;

    for rp in get_patterns() {
        if rp.pattern.is_match(&output) {
            triggered = true;
            // Rust's regex replace_all replaces matches
            output = rp.pattern.replace_all(&output, REDACTED).into_owned();
        }
    }

    RedactResult { output, triggered }
}

pub fn redact_strict(text: &str) -> String {
    redact(text).output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redact_db_urls() {
        let input = "postgresql://user:pass@localhost:5432/db";
        let res = redact(input);
        assert!(res.triggered);
        assert_eq!(res.output, "[REDACTED_SECRET]");
    }

    #[test]
    fn test_redact_github_pat() {
        let input = "ghp_123456789012345678901234567890123456";
        let res = redact(input);
        assert!(res.triggered);
        assert_eq!(res.output, "[REDACTED_SECRET]");
    }

    #[test]
    fn test_redact_openai_key() {
        let input = "sk-12345678901234567890123456789012";
        let res = redact(input);
        assert!(res.triggered);
        assert_eq!(res.output, "[REDACTED_SECRET]");
    }

    #[test]
    fn test_redact_generic_secret() {
        let input = "api_key = \"mysecretkey123\"";
        let res = redact(input);
        assert!(res.triggered);
        assert_eq!(res.output, "[REDACTED_SECRET]");
    }

    #[test]
    fn test_redact_private_key() {
        let input = "-----BEGIN PRIVATE KEY-----\nMIIEvgIBADANBgkqhkiG9w0BAQEFAASCBKgwggSkAgEAAoIBAQC7\n-----END PRIVATE KEY-----";
        let res = redact(input);
        assert!(res.triggered);
        assert_eq!(res.output, "[REDACTED_SECRET]");
    }
}
