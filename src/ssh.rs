use std::{ffi::OsString, path::Path};

pub const SSH_PROGRAM: &str = "ssh";

#[must_use]
pub const fn is_ssh_error_exit_code(exit_code: i64) -> bool {
    exit_code == 255
}

#[must_use]
pub fn ssh_arguments(config: &Path, alias: &str) -> Vec<OsString> {
    vec![
        OsString::from("-F"),
        config.as_os_str().to_owned(),
        OsString::from("--"),
        OsString::from(alias),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_use_the_loaded_config() {
        assert_eq!(
            ssh_arguments(Path::new("/tmp/alternate-config"), "work-web"),
            ["-F", "/tmp/alternate-config", "--", "work-web"].map(OsString::from)
        );
    }

    #[test]
    fn only_255_is_an_ssh_error_exit_code() {
        assert!(!is_ssh_error_exit_code(0));
        assert!(!is_ssh_error_exit_code(1));
        assert!(!is_ssh_error_exit_code(254));
        assert!(is_ssh_error_exit_code(255));
    }
}
