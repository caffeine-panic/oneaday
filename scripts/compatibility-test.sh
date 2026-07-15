#!/usr/bin/env bash

set -Eeuo pipefail

protocol="${1:-}"
version="${2:-}"
nacos_api="${3:-}"

if [[ -z "$protocol" || -z "$version" ]]; then
  echo "usage: $0 <etcd|zookeeper|nacos> <version> [v2|v3]" >&2
  exit 2
fi

case "$protocol" in
  etcd | zookeeper | nacos) ;;
  *)
    echo "unsupported protocol: $protocol" >&2
    exit 2
    ;;
esac

if [[ "$protocol" == "nacos" && "$nacos_api" != "v2" && "$nacos_api" != "v3" ]]; then
  echo "Nacos compatibility tests require an explicit v2 or v3 API profile" >&2
  exit 2
fi

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
container_name="atlas-compat-${protocol}-${version//[^[:alnum:]]/-}-$$"

cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ $status -ne 0 ]] && docker inspect "$container_name" >/dev/null 2>&1; then
    echo "--- container logs: $container_name ---" >&2
    docker logs --tail 300 "$container_name" >&2 || true
  fi
  docker rm -f "$container_name" >/dev/null 2>&1 || true
  exit "$status"
}
trap cleanup EXIT INT TERM

if ! docker info >/dev/null 2>&1; then
  echo "Docker daemon is unavailable" >&2
  exit 1
fi

wait_for_command() {
  local description="$1"
  shift
  for _ in $(seq 1 90); do
    if "$@" >/dev/null 2>&1; then
      return 0
    fi
    sleep 2
  done
  echo "$description did not become ready within 180 seconds" >&2
  return 1
}

run_live_test() {
  local test_name="$1"
  shift
  (
    cd "$repository_root"
    env "$@" cargo test --manifest-path src-tauri/Cargo.toml \
      --test live_registry "$test_name" -- --ignored --exact --nocapture
  )
}

run_etcd() {
  docker run --detach --name "$container_name" \
    --publish 127.0.0.1:2379:2379 \
    "${ATLAS_ETCD_IMAGE_REPOSITORY:-quay.io/coreos/etcd}:v${version}" \
    /usr/local/bin/etcd \
    --name atlas-compat \
    --data-dir /tmp/etcd-data \
    --listen-client-urls http://0.0.0.0:2379 \
    --advertise-client-urls http://0.0.0.0:2379 \
    --listen-peer-urls http://0.0.0.0:2380 >/dev/null

  wait_for_command "etcd $version" docker exec "$container_name" \
    /usr/local/bin/etcdctl endpoint health

  local lease_id
  lease_id="$(docker exec "$container_name" /usr/local/bin/etcdctl lease grant 600 | awk '{print $2}')"
  if [[ -z "$lease_id" ]]; then
    echo "failed to create etcd lease fixture" >&2
    return 1
  fi
  docker exec "$container_name" /usr/local/bin/etcdctl put \
    /atlas/fixture 'atlas-etcd-fixture' --lease="$lease_id" >/dev/null

  run_live_test etcd_live_session_can_browse_the_root \
    ATLAS_TEST_ETCD_ENDPOINT=127.0.0.1:2379 \
    ATLAS_TEST_ETCD_KEY=/atlas/fixture \
    ATLAS_TEST_ETCD_MUTATION_PREFIX=/atlas-registry-tests \
    ATLAS_TEST_ENABLE_MUTATIONS=1
}

run_zookeeper() {
  docker run --detach --name "$container_name" \
    --publish 127.0.0.1:2181:2181 \
    --env ZOO_4LW_COMMANDS_WHITELIST=ruok \
    "${ATLAS_ZOOKEEPER_IMAGE_REPOSITORY:-zookeeper}:${version}" >/dev/null

  wait_for_command "ZooKeeper $version" docker exec "$container_name" \
    zkCli.sh -server 127.0.0.1:2181 ls /

  docker exec "$container_name" zkCli.sh -server 127.0.0.1:2181 \
    create /atlas /atlas >/dev/null
  docker exec "$container_name" zkCli.sh -server 127.0.0.1:2181 \
    create /atlas/fixture atlas-zookeeper-fixture >/dev/null
  docker exec "$container_name" zkCli.sh -server 127.0.0.1:2181 \
    create /atlas-registry-tests atlas-registry-tests >/dev/null

  run_live_test zookeeper_live_session_can_browse_the_root \
    ATLAS_TEST_ZOOKEEPER_ENDPOINT=127.0.0.1:2181 \
    ATLAS_TEST_ZOOKEEPER_PATH=/atlas/fixture \
    ATLAS_TEST_ZOOKEEPER_MUTATION_PARENT=/atlas-registry-tests \
    ATLAS_TEST_ENABLE_MUTATIONS=1
}

nacos_ready() {
  if [[ "$nacos_api" == "v2" ]]; then
    curl --fail --silent --show-error --max-time 3 \
      http://127.0.0.1:8848/nacos/v1/console/health/readiness >/dev/null
  else
    curl --fail --silent --show-error --max-time 3 \
      http://127.0.0.1:8848/nacos/v3/admin/core/state/readiness >/dev/null
  fi
}

publish_nacos_fixture() {
  local content="$1"
  if [[ "$nacos_api" == "v2" ]]; then
    curl --fail --silent --show-error --max-time 10 \
      --request POST http://127.0.0.1:8848/nacos/v1/cs/configs \
      --data-urlencode dataId=atlas-fixture.yaml \
      --data-urlencode group=DEFAULT_GROUP \
      --data-urlencode tenant= \
      --data-urlencode "content=$content" >/dev/null
  else
    curl --fail --silent --show-error --max-time 10 \
      --request POST http://127.0.0.1:8848/nacos/v3/admin/cs/config \
      --data-urlencode namespaceId=public \
      --data-urlencode groupName=DEFAULT_GROUP \
      --data-urlencode dataId=atlas-fixture.yaml \
      --data-urlencode "content=$content" >/dev/null
  fi
}

run_nacos() {
  local image_suffix=""
  if [[ "$(docker info --format '{{.Architecture}}')" == "aarch64" ]]; then
    image_suffix="-slim"
  fi
  docker run --detach --name "$container_name" \
    --publish 127.0.0.1:8848:8848 \
    --publish 127.0.0.1:9848:9848 \
    --publish 127.0.0.1:9849:9849 \
    --env MODE=standalone \
    --env NACOS_AUTH_ENABLE=false \
    --env NACOS_AUTH_ADMIN_ENABLE=false \
    --env NACOS_AUTH_CONSOLE_ENABLE=false \
    --env NACOS_AUTH_IDENTITY_KEY=atlas-compat-key \
    --env NACOS_AUTH_IDENTITY_VALUE=atlas-compat-value \
    --env NACOS_AUTH_TOKEN=QXRsYXNSZWdpc3RyeUNvbXBhdGliaWxpdHlUZXN0VG9rZW4xMjM0NTY3OA== \
    "${ATLAS_NACOS_IMAGE_REPOSITORY:-nacos/nacos-server}:v${version}${image_suffix}" >/dev/null

  wait_for_command "Nacos $version" nacos_ready
  publish_nacos_fixture 'version: 1'
  sleep 1
  publish_nacos_fixture 'version: 2'

  run_live_test nacos_live_session_can_browse_the_config_list \
    ATLAS_TEST_NACOS_ENDPOINT=127.0.0.1:8848 \
    ATLAS_TEST_NACOS_VERSION="$nacos_api" \
    ATLAS_TEST_NACOS_NAMESPACE=public \
    ATLAS_TEST_NACOS_GROUP=DEFAULT_GROUP \
    ATLAS_TEST_NACOS_DATA_ID=atlas-fixture.yaml \
    ATLAS_TEST_NACOS_MUTATION_GROUP=ATLAS_REGISTRY_TEST \
    ATLAS_TEST_ENABLE_MUTATIONS=1
}

case "$protocol" in
  etcd) run_etcd ;;
  zookeeper) run_zookeeper ;;
  nacos) run_nacos ;;
esac
