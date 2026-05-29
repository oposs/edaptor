#!/usr/bin/env bash
# Start/stop a throwaway OpenLDAP server for edaptor integration tests (podman).
# Usage: scripts/test-ldap.sh [start|stop]
set -euo pipefail

NAME=edaptor-test-ldap
# Bitnami migrated free images from bitnami/ to bitnamilegacy/ in Aug 2025.
# bitnamilegacy/openldap is the same image at its new vendor-assigned address.
IMAGE=docker.io/bitnamilegacy/openldap:2.6.9

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
      "$IMAGE" >/dev/null
    echo "Waiting for LDAP to accept connections..."
    for _ in $(seq 1 30); do
      if podman exec "$NAME" ldapsearch -x -H ldap://localhost:1389 \
           -b "dc=example,dc=org" -s base >/dev/null 2>&1; then
        echo "Ready."
        echo "  export EDAPTOR_TEST_LDAP_URI=ldap://localhost:1389"
        echo "  export EDAPTOR_TEST_ADMIN_PW=adminpassword"
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
