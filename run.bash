#!/usr/bin/env bash

set -o errexit -o pipefail -o nounset

cd "$(dirname "${BASH_SOURCE[0]}")"

cargo build

exec env RUST_LOG="${RUST_LOG:-bitdemon=trace,dw_server=trace}" \
     ./target/debug/dw-server "$@"
