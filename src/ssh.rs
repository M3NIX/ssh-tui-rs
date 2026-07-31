use std::{ffi::OsString, path::Path};

pub const SSH_PROGRAM: &str = "ssh";

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
}
