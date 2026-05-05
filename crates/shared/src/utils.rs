//! Small helpers shared across crates.

/// Parse `--flag value` from the arg list, returning `default` if not found.
///
/// Used by binary entry points to extract optional CLI overrides without
/// pulling in `clap` for one-off flags.
pub fn parse_arg<T: std::str::FromStr>(args: &[String], flag: &str, default: T) -> T {
    args.windows(2)
        .find(|w| w[0] == flag)
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(default)
}
