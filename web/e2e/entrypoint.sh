#!/bin/bash
# The repository is mounted from the host, so the files the run writes into it -- the
# recorded screenshots and the report -- have to end up owned by whoever started the
# container. Docker cannot look up that user id itself, so take it from the mount.
set -euo pipefail

if [[ $(id -u) == 0 ]]; then
    owner=$(stat -c '%u:%g' .)
    exec setpriv --reuid "${owner%%:*}" --regid "${owner##*:}" --clear-groups "$@"
fi

exec "$@"
