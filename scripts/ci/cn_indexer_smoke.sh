#!/usr/bin/env bash
# Production indexer startup contract; accepts the image checked by cn_image_check.py.
set -euo pipefail
image="${1:?usage: cn_indexer_smoke.sh IMAGE}"
# secret 実値は使わない。SENTINEL はダミー credential の漏出検知マーカー。
base_args=(
  --rm
  -e COMMUNITY_NODE_DATABASE_URL=postgres://smoke.invalid/cn
  -e COMMUNITY_NODE_CHANNEL_SECRET_KEY=image-smoke-channel-secret-key-0123456789abcdef
  -e COMMUNITY_NODE_INDEXER_EXTERNAL_RELAY_URLS=https://relay.example.net
  -e COMMUNITY_NODE_INDEXER_RETENTION_DAYS=30
  -e COMMUNITY_NODE_INDEXER_CAPACITY_ROWS=1000000
)
leak_log=""

run_case() {
  local name="$1" expected="$2" pattern="$3"
  shift 3
  local out status
  set +e
  out=$(docker run "${base_args[@]}" "$@" "$image" validate-config 2>&1)
  status=$?
  set -e
  leak_log+="$out"$'\n'
  if [[ "$expected" == "ok" && "$status" -ne 0 ]]; then
    echo "::error::smoke case '$name' expected exit 0 but got $status"
    echo "$out"
    exit 1
  fi
  if [[ "$expected" == "fail" && "$status" -eq 0 ]]; then
    echo "::error::smoke case '$name' expected a startup failure but got exit 0"
    echo "$out"
    exit 1
  fi
  if [[ -n "$pattern" ]] && ! grep -qF "$pattern" <<<"$out"; then
    echo "::error::smoke case '$name' output does not contain: $pattern"
    echo "$out"
    exit 1
  fi
  echo "smoke case '$name': ok"
}

# 正常系: provider 未構成は ingest 無効で常駐する正規の fail-closed 状態。
run_case "provider-less config" ok "cn-indexer configuration is valid"

# 正常系: production provider 構成（ダミー credential で構成検証のみ）。
run_case "production providers" ok "cn-indexer configuration is valid" \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM=project-arachnid-shield \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL=openai-compatible-vlm \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_UNKNOWN_CSAM=openai-compatible-vlm \
  -e PROJECT_ARACHNID_API_USERNAME=dummy-arachnid-user-SENTINEL \
  -e PROJECT_ARACHNID_API_PASSWORD=dummy-arachnid-password-SENTINEL \
  -e COMMUNITY_NODE_VLM_API_BASE_URL=http://vlm.smoke.invalid:8000 \
  -e COMMUNITY_NODE_VLM_MODEL=dummy-org/dummy-model \
  -e COMMUNITY_NODE_VLM_API_KEY=dummy-vlm-key-SENTINEL \
  -e COMMUNITY_NODE_SAFETY_EMIT_SIGNED_EVENTS=false \
  -e COMMUNITY_NODE_SAFETY_ISSUER_NODE_ID=smoke-issuer

# mock は production image で構造的に選択不能（unknown provider として fail-closed）。
run_case "mock rejected" fail 'unknown safety provider `mock`' \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM=mock

# 未知 provider 名は fail-closed で起動失敗。
run_case "unknown provider rejected" fail "unknown safety provider" \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM=no-such-provider

# slot 制約: VLM は general / unknown_csam のみ、Arachnid は known_csam のみ。
run_case "vlm rejected on known_csam slot" fail 'only supports the `general` / `unknown_csam` slots' \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM=openai-compatible-vlm
run_case "arachnid rejected on general slot" fail 'only supports the `known_csam` slot' \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL=project-arachnid-shield

# credential / endpoint / 署名鍵の欠落は env 名を明示して起動失敗。
run_case "missing arachnid credentials" fail "PROJECT_ARACHNID_API_USERNAME" \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM=project-arachnid-shield
run_case "missing vlm endpoint" fail "COMMUNITY_NODE_VLM_API_BASE_URL" \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_GENERAL=openai-compatible-vlm
run_case "missing signing key" fail "no signing key is configured" \
  -e COMMUNITY_NODE_SAFETY_PROVIDER_KNOWN_CSAM=project-arachnid-shield \
  -e PROJECT_ARACHNID_API_USERNAME=dummy-arachnid-user-SENTINEL \
  -e PROJECT_ARACHNID_API_PASSWORD=dummy-arachnid-password-SENTINEL

# どのケースでも credential 値（SENTINEL 付きダミー値）が出力へ漏れないこと。
if grep -qF "SENTINEL" <<<"$leak_log"; then
  echo "::error::a credential value leaked into the smoke output"
  exit 1
fi
echo "credential leak check: ok"
