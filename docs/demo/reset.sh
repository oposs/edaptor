#!/bin/bash
# Reset the `readers` demo group so the tour records identically each run.
set -e
H=ldap://localhost:1389
D="cn=admin,dc=example,dc=org"
W=adminpassword
ldapmodify -x -H "$H" -D "$D" -w "$W" -c >/dev/null 2>&1 <<LDIF || true
dn: cn=readers,ou=groups,dc=example,dc=org
changetype: modify
replace: member
member: cn=user01,ou=users,dc=example,dc=org
member: cn=user02,ou=users,dc=example,dc=org
-
replace: description
LDIF
echo "reset: readers → 2 members, no description"
