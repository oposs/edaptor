#!/usr/bin/env bash
# Start/stop a throwaway OpenLDAP server for edaptor integration tests.
# Works with podman or docker (auto-detected, podman preferred).
# Usage: scripts/test-ldap.sh [--engine podman|docker] [start|stop]
#   The engine can also be selected with EDAPTOR_CONTAINER_ENGINE=docker.
set -euo pipefail

NAME=edaptor-test-ldap
# Bitnami migrated free images from bitnami/ to bitnamilegacy/ in Aug 2025.
# bitnamilegacy/openldap is the same image at its new vendor-assigned address.
IMAGE=docker.io/bitnamilegacy/openldap:2.6.9

PROVISION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/ldap-provision" && pwd)"

apply_ldif() {  # <bind-dn> <password> <file>
  local bind_dn=$1 pw=$2 file=$3 base out rc
  base=$(basename "$file")
  "$ENGINE" cp "$file" "$NAME:/tmp/$base" || return 1
  # -c keeps going past entries that already exist (idempotent re-runs, and the
  # Bitnami default tree already owns e.g. ou=groups). ldapadd then exits
  # non-zero, which under `set -e` would abort provisioning — so we capture the
  # output + exit code and tolerate ONLY the benign "Already exists (68)".
  if out=$("$ENGINE" exec "$NAME" ldapadd -c -x -H ldap://localhost:1389 \
       -D "$bind_dn" -w "$pw" -f "/tmp/$base" 2>&1); then
    rc=0
  else
    rc=$?
  fi
  echo "$out"
  if [ "$rc" -ne 0 ]; then
    # Any tool error line (ldap_add/ldap_bind/ldap_modify/ldapadd:, etc.) that
    # is NOT "Already exists (68)" is fatal — this also catches bind/connection
    # failures, which produce ldap_bind:/ldapadd: lines (not ldap_add:).
    if echo "$out" | grep -E '^(ldap_|ldapadd:)' | grep -qvF 'Already exists (68)'; then
      echo "ERROR: ldapadd failed for $base (see output above)" >&2
      return 1
    fi
    # Non-zero exit with no recognizable error line at all is also fatal.
    if ! echo "$out" | grep -qF 'Already exists (68)'; then
      echo "ERROR: ldapadd failed for $base with no recoverable error (rc=$rc)" >&2
      return 1
    fi
  fi
}

# The image's setup runs a temporary slapd on the same port while it builds the
# config and the default tree, then stops it and starts the real one. A probe
# that reaches the temporary server passes too early, and provisioning then
# dies with "Can't contact LDAP server" once setup stops it. So wait for the
# image's own "Starting slapd" line (setup finished) before probing.
ldap_ready() {
  # grep without -q reads all input: an early exit would SIGPIPE `logs`, and
  # pipefail would then report the match as a failure.
  "$ENGINE" logs "$NAME" 2>&1 | grep -F '** Starting slapd **' >/dev/null &&
    "$ENGINE" exec "$NAME" ldapsearch -x -H ldap://localhost:1389 \
      -b "dc=example,dc=org" -s base >/dev/null 2>&1
}

# Provisioning must leave the seed data behind; check one seeded entry.
seeded() {
  "$ENGINE" exec "$NAME" ldapsearch -x -H ldap://localhost:1389 \
    -D "cn=admin,dc=example,dc=org" -w adminpassword \
    -b "uid=jsmith,ou=people,dc=example,dc=org" -s base dn >/dev/null 2>&1
}

# Fail loudly and leave no half-provisioned server running for tests to hit.
fail() {
  echo "ERROR: $*" >&2
  "$ENGINE" stop "$NAME" >/dev/null 2>&1 || true
  exit 1
}

provision() {
  echo "Provisioning schemas + overlays (cn=config admin)..."
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/schema/samba.ldif" || return 1
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/schema/mail.ldif" || return 1
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/config/overlays.ldif" || return 1
  echo "Loading directory data (data admin)..."
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/ppolicy.ldif" || return 1
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/base.ldif" || return 1
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/testdata.ldif" || return 1
}

# Parse args: an optional --engine override plus the start|stop command.
ENGINE="${EDAPTOR_CONTAINER_ENGINE:-}"
CMD=""
usage() { echo "usage: $0 [--engine podman|docker] [start|stop]"; }
while [ $# -gt 0 ]; do
  case "$1" in
    --engine) ENGINE="${2:-}"; shift 2 ;;
    --engine=*) ENGINE="${1#*=}"; shift ;;
    start|stop) CMD="$1"; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 1 ;;
  esac
done
CMD="${CMD:-start}"

# Resolve the container engine: honour an explicit choice, else prefer podman
# and fall back to docker (the OpenLDAP image + cp/exec/run/stop calls work the
# same on both).
if [ -n "$ENGINE" ]; then
  command -v "$ENGINE" >/dev/null 2>&1 || {
    echo "ERROR: requested container engine '$ENGINE' not found in PATH" >&2; exit 1; }
else
  for _e in podman docker; do
    if command -v "$_e" >/dev/null 2>&1; then ENGINE="$_e"; break; fi
  done
  [ -n "$ENGINE" ] || {
    echo "ERROR: neither podman nor docker found in PATH" >&2; exit 1; }
fi

case "$CMD" in
  start)
    echo "Using container engine: $ENGINE"
    # Make start idempotent: clear any leftover container from a prior run
    # (e.g. one left behind by a readiness timeout below).
    "$ENGINE" rm -f "$NAME" >/dev/null 2>&1 || true
    "$ENGINE" run -d --rm --name "$NAME" \
      -p 11389:1389 \
      -e LDAP_ROOT="dc=example,dc=org" \
      -e LDAP_ADMIN_USERNAME="admin" \
      -e LDAP_ADMIN_PASSWORD="adminpassword" \
      -e LDAP_CONFIG_ADMIN_ENABLED="yes" \
      -e LDAP_CONFIG_ADMIN_USERNAME="admin" \
      -e LDAP_CONFIG_ADMIN_PASSWORD="configpassword" \
      "$IMAGE" >/dev/null
    echo "Waiting for LDAP to accept connections..."
    for _ in $(seq 1 60); do
      if ldap_ready; then
        echo "Ready."
        provision || fail "provisioning failed; server stopped"
        seeded || fail "provisioning left no seed data (uid=jsmith missing); server stopped"
        echo "Provisioned. Connection hints:"
        echo "  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:11389"
        echo "  export EDAPTOR_TEST_ADMIN_PW=adminpassword"
        echo "  edaptor --config examples/demo-config.toml   # explore the seed data"
        exit 0
      fi
      sleep 1
    done
    fail "LDAP did not become ready in time"
    ;;
  stop)
    "$ENGINE" stop "$NAME" >/dev/null 2>&1 || true
    echo "Stopped $NAME"
    ;;
  *)
    usage >&2
    exit 1
    ;;
esac
