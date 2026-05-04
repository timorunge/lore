use std::borrow::Cow;
use std::sync::LazyLock;

use regex::Regex;

/// Regex matching `$VAR` and `${VAR}` environment variable references.
static ENV_VAR_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([a-zA-Z_][a-zA-Z0-9_]*)\}|\$([a-zA-Z_][a-zA-Z0-9_]*)").unwrap()
});

/// Expand `$VAR` and `${VAR}` references in a string.
///
/// - `warn_missing`: log when a referenced var is not set.
/// - `allowed_prefix`: if `Some("LORE_")`, only expand vars matching that prefix;
///   non-matching vars are left unexpanded with a warning. Pass `Some("LORE_")` for
///   header values to prevent leakage of sensitive variables such as
///   `AWS_SECRET_ACCESS_KEY` into outbound HTTP requests.
///
/// Security note: CRLF injection via expanded values in HTTP headers is mitigated
/// by reqwest's header validation, which rejects header values containing `\r` or `\n`.
#[allow(clippy::type_complexity)]
pub(super) fn replace_env_vars<'a>(
    s: &'a str,
    warn_missing: bool,
    allowed_prefix: Option<&str>,
    env_fn: Option<&dyn Fn(&str) -> Option<String>>,
) -> Cow<'a, str> {
    ENV_VAR_RE.replace_all(s, |caps: &regex::Captures| {
        let var_name = caps
            .get(1)
            .or_else(|| caps.get(2))
            .expect("regex requires one of two capture groups to match")
            .as_str();
        if let Some(prefix) = allowed_prefix
            && !var_name.starts_with(prefix)
        {
            tracing::warn!(
                var = var_name,
                "only {prefix}* env vars are expanded in headers; leaving ${{{var_name}}} unexpanded"
            );
            return caps[0].to_owned();
        }
        let resolved = if let Some(f) = env_fn {
            f(var_name)
        } else {
            std::env::var(var_name).ok()
        };
        if let Some(val) = resolved {
            val
        } else {
            if warn_missing {
                tracing::warn!(
                    var = var_name,
                    "environment variable not set, substituting empty string"
                );
            }
            String::new()
        }
    })
}

/// Expand `~` and `$VAR` / `${VAR}` references in a filesystem path.
///
/// Note: only `~/` and bare `~` are expanded; `~username` is not supported.
///
/// All environment variables are expanded (no prefix restriction), unlike header
/// expansion which limits substitution to `LORE_*` vars to prevent accidental
/// exfiltration of sensitive environment variables.
pub fn expand_path(s: &str) -> String {
    let mut result = s.to_owned();

    if (result.starts_with("~/") || result == "~")
        && let Some(home) = dirs::home_dir()
    {
        result = if result == "~" {
            home.to_string_lossy().into_owned()
        } else {
            home.join(&result[2..]).to_string_lossy().into_owned()
        };
    }

    replace_env_vars(&result, true, None, None).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_cases() {
        // expand_path: tilde, tilde+subdir, absolute, relative, $VAR
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_path("~"), home.to_string_lossy().as_ref());

        let result = expand_path("~/subdir");
        let expected_suffix = std::path::Path::new("subdir");
        assert!(
            std::path::Path::new(&result).ends_with(expected_suffix),
            "result {result:?} should end with 'subdir'"
        );
        assert!(
            result.starts_with(home.to_string_lossy().as_ref()),
            "result {result:?} should start with home dir"
        );

        assert_eq!(expand_path("relative/path"), "relative/path");

        // SAFETY: test-only, unique var name prevents races with other tests.
        unsafe { std::env::set_var("LORE_TEST_EXPAND_HOME_1", "testval") };
        assert_eq!(expand_path("$LORE_TEST_EXPAND_HOME_1/docs"), "testval/docs");

        // replace_env_vars: bare var, braced var, unset var, allowed prefix,
        // disallowed prefix
        let env_1 = |name: &str| -> Option<String> {
            (name == "LORE_TEST_EXPAND_1").then(|| "hello".to_owned())
        };
        assert_eq!(
            replace_env_vars("$LORE_TEST_EXPAND_1 world", false, None, Some(&env_1)),
            "hello world"
        );

        let env_2 = |name: &str| -> Option<String> {
            (name == "LORE_TEST_EXPAND_2").then(|| "braced".to_owned())
        };
        assert_eq!(
            replace_env_vars("${LORE_TEST_EXPAND_2}", false, None, Some(&env_2)),
            "braced"
        );

        let env_none = |_name: &str| -> Option<String> { None };
        assert_eq!(
            replace_env_vars(
                "prefix-$LORE_TEST_EXPAND_UNSET_99-suffix",
                false,
                None,
                Some(&env_none),
            ),
            "prefix--suffix"
        );

        let env_3 = |name: &str| -> Option<String> {
            (name == "LORE_TEST_EXPAND_3").then(|| "allowed".to_owned())
        };
        assert_eq!(
            replace_env_vars(
                "$LORE_TEST_EXPAND_3",
                false,
                Some("LORE_TEST_"),
                Some(&env_3)
            ),
            "allowed"
        );

        let env_4 = |name: &str| -> Option<String> {
            (name == "LORE_TEST_EXPAND_4").then(|| "secret".to_owned())
        };
        assert_eq!(
            replace_env_vars(
                "$LORE_TEST_EXPAND_4",
                false,
                Some("LORE_OTHER_"),
                Some(&env_4)
            ),
            "$LORE_TEST_EXPAND_4"
        );
    }
}
