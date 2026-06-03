#!/usr/bin/env bash
# Start/stop a throwaway OpenLDAP server for edaptor integration tests (podman).
# Usage: scripts/test-ldap.sh [start|stop]
set -euo pipefail

NAME=edaptor-test-ldap
# Bitnami migrated free images from bitnami/ to bitnamilegacy/ in Aug 2025.
# bitnamilegacy/openldap is the same image at its new vendor-assigned address.
IMAGE=docker.io/bitnamilegacy/openldap:2.6.9

PROVISION_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/ldap-provision" && pwd)"

apply_ldif() {  # <bind-dn> <password> <file>
  local bind_dn=$1 pw=$2 file=$3 base out
  base=$(basename "$file")
  podman cp "$file" "$NAME:/tmp/$base"
  # -c keeps going past entries that already exist (idempotent re-runs, and the
  # Bitnami default tree already owns e.g. ou=groups). ldapadd then exits
  # non-zero, which under `set -e` would abort provisioning — so we capture the
  # output and tolerate only "Already exists (68)"; any other ldap_add error
  # is surfaced and fails loudly.
  out=$(podman exec "$NAME" ldapadd -c -x -H ldap://localhost:1389 \
    -D "$bind_dn" -w "$pw" -f "/tmp/$base" 2>&1) || true
  echo "$out"
  if echo "$out" | grep '^ldap_add:' | grep -qv 'Already exists (68)'; then
    echo "ERROR: ldapadd reported a non-recoverable error for $base" >&2
    return 1
  fi
}

provision() {
  echo "Provisioning schemas + overlays (cn=config admin)..."
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/schema/samba.ldif"
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/schema/mail.ldif"
  apply_ldif "cn=admin,cn=config" "configpassword" "$PROVISION_DIR/config/overlays.ldif"
  echo "Loading directory data (data admin)..."
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/ppolicy.ldif"
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/base.ldif"
  apply_ldif "cn=admin,dc=example,dc=org" "adminpassword" "$PROVISION_DIR/data/testdata.ldif"
}

case "${1:-start}" in
  start)
    # Make start idempotent: clear any leftover container from a prior run
    # (e.g. one left behind by a readiness timeout below).
    podman rm -f "$NAME" >/dev/null 2>&1 || true
    podman run -d --rm --name "$NAME" \
      -p 1389:1389 \
      -e LDAP_ROOT="dc=example,dc=org" \
      -e LDAP_ADMIN_USERNAME="admin" \
      -e LDAP_ADMIN_PASSWORD="adminpassword" \
      -e LDAP_CONFIG_ADMIN_ENABLED="yes" \
      -e LDAP_CONFIG_ADMIN_USERNAME="admin" \
      -e LDAP_CONFIG_ADMIN_PASSWORD="configpassword" \
      "$IMAGE" >/dev/null
    echo "Waiting for LDAP to accept connections..."
    for _ in $(seq 1 30); do
      if podman exec "$NAME" ldapsearch -x -H ldap://localhost:1389 \
           -b "dc=example,dc=org" -s base >/dev/null 2>&1; then
        echo "Ready."
        provision
        echo "Provisioned. Connection hints:"
        echo "  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389"
        echo "  export EDAPTOR_TEST_ADMIN_PW=adminpassword"
        echo "  edaptor --config examples/demo-config.toml   # explore the seed data"
        exit 0
      fi
      sleep 1
    done
    echo "ERROR: LDAP did not become ready in time" >&2
    podman stop "$NAME" >/dev/null 2>&1 || true
    exit 1
    ;;
  stop)
    podman stop "$NAME" >/dev/null 2>&1 || true
    echo "Stopped $NAME"
    ;;
  *)
    echo "usage: $0 [start|stop]" >&2
    exit 1
    ;;
esac
