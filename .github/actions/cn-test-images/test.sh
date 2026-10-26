#!/usr/bin/env bash
set -euo pipefail

action_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
action=$(sed -n '/      run: |/,$s/^        //p' "$action_dir/action.yml")
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
export PULL_LOG="$work/docker.log"

docker() {
  echo "$*" >> "$PULL_LOG"
  if [[ "$1" == pull && "$2" == "mirror.gcr.io/${FAILED_IMAGE:-}" ]]; then
    echo 'unknown blob' >&2
    return 1
  fi
  if [[ "$1" == pull && "$2" == "${FAILED_IMAGE:-}" && "${FAIL_SOURCE:-}" == true ]]; then
    return 1
  fi
}
export -f docker

for failed_image in '' library/postgres:17-bookworm valkey/valkey:8-alpine; do
  export FAILED_IMAGE="$failed_image"
  : > "$PULL_LOG"
  bash -euo pipefail <<< "$action"
  expected=''
  for image in library/postgres:17-bookworm valkey/valkey:8-alpine; do
    expected+="pull mirror.gcr.io/$image"$'\n'
    if [[ "$image" == "$failed_image" ]]; then
      expected+="pull $image"$'\n'
    else
      expected+="tag mirror.gcr.io/$image ${image#library/}"$'\n'
    fi
  done
  diff -u <(printf '%s' "$expected") "$PULL_LOG"
done

export FAIL_SOURCE=true
for image in library/postgres:17-bookworm valkey/valkey:8-alpine; do
  export FAILED_IMAGE="$image"
  : > "$PULL_LOG"
  if bash -euo pipefail <<< "$action"; then
    echo 'Both registries failed but the action succeeded' >&2
    exit 1
  fi
  [[ "$(tail -n 1 "$PULL_LOG")" == "pull $image" ]]
done
echo 'CN test image pull contract: PASS (5 cases)'
