# Test-only adapter that generates schema-1 YAML fixtures from a Fish command.
# Production Jobflick rejects raw shell submissions.
submit_execarium() {
  local command="$1"
  {
    printf 'schema: 1\nshell: fish\ncwd: %s\nprivilege: none\ntimeout_seconds: 120\ncommand: |\n' "$PWD"
    while IFS= read -r line || [[ -n "$line" ]]; do
      printf '  %s\n' "$line"
    done <<< "$command"
  } | "$bin" submit-spec -
}
