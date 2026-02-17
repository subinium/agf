pub fn shell_init(shell: &str) -> String {
    match shell {
        "zsh" => ZSH_WRAPPER.to_string(),
        "bash" => BASH_WRAPPER.to_string(),
        "fish" => FISH_WRAPPER.to_string(),
        other => format!("echo \"Unsupported shell: {other}. Use zsh, bash, or fish.\""),
    }
}

const ZSH_WRAPPER: &str = r#"function agf() {
    local result
    result="$(command agf "$@")"
    if [ $? -eq 0 ] && [ -n "$result" ]; then
        eval "$result"
    fi
}"#;

const BASH_WRAPPER: &str = r#"function agf() {
    local result
    result="$(command agf "$@")"
    if [ $? -eq 0 ] && [ -n "$result" ]; then
        eval "$result"
    fi
}"#;

const FISH_WRAPPER: &str = r#"function agf
    set -l result (command agf $argv)
    if test $status -eq 0; and test -n "$result"
        eval $result
    end
end"#;
