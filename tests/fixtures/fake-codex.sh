#!/bin/sh
set -eu

[ "$1" = app-server ] && [ "$2" = --stdio ] || exit 1

read_request() {
    IFS= read -r request
    id=$(printf '%s\n' "$request" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
    [ -n "$id" ]
}

reply() {
    printf '{"id":%s,"result":%s}\n' "$id" "$1"
}

read_request
case $request in *'"method":"initialize"'*) ;; *) exit 1 ;; esac
reply '{}'
IFS= read -r initialized
case $initialized in *'"method":"initialized"'*) ;; *) exit 1 ;; esac

read_request
events="$CODEX_HOME/discovery/events"
if [ -f "$events" ]; then
    printf 'start\n' >> "$events"
    while [ -f "$CODEX_HOME/discovery/hold" ]; do sleep 0.01; done
fi
printf '%s\n' "$$" >> "$CODEX_HOME/requests"
if [ -e "$CODEX_HOME/inherit-stderr" ]; then
    sleep 2 </dev/null >/dev/null &
fi
if [ -p "$CODEX_HOME/release" ]; then
    IFS= read -r release < "$CODEX_HOME/release"
fi
if [ -f "$CODEX_HOME/stderr" ]; then
    cat "$CODEX_HOME/stderr" >&2
    exit 1
fi
if [ -f "$CODEX_HOME/rpc-error.json" ]; then
    printf '{"id":%s,"error":%s}\n' "$id" "$(cat "$CODEX_HOME/rpc-error.json")"
    exit
fi

if [ -f "$events" ]; then printf 'end\n' >> "$events"; fi
case $request in
    *'"method":"account/rateLimits/read"'*)
        reply '{"rateLimits":{"limitId":"codex","primary":{"usedPercent":12,"windowDurationMins":300},"secondary":{"usedPercent":34,"windowDurationMins":10080}},"rateLimitResetCredits":{"availableCount":2,"credits":[{"status":"available","expiresAt":1784246400}]}}'
        ;;
    *'"method":"account/read"'*)
        case $request in *'"refreshToken":false'*) ;; *) exit 1 ;; esac
        reply '{"account":{"type":"chatgpt","email":"test@example.com","planType":"plus"}}'
        ;;
    *) exit 1 ;;
esac
