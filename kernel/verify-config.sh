#!/usr/bin/env bash
# Checks that every line of every config fragment survived olddefconfig, so a
# dropped dependency fails the build instead of silently changing the kernel.
# A "not set" line passes if the option is off or no longer offered.
#
#   kernel/verify-config.sh <.config> <fragment>...
set -euo pipefail

config=$1
shift
bad=0
while IFS= read -r line; do
	if [[ $line =~ ^(CONFIG_[A-Za-z0-9_]+)= ]]; then
		if ! grep -qxF -- "$line" "$config"; then
			echo "kernel config: wanted $line, got: $(grep -E "^(# )?${BASH_REMATCH[1]}[= ]" "$config" || echo missing)" >&2
			bad=1
		fi
	elif [[ $line =~ ^#\ (CONFIG_[A-Za-z0-9_]+)\ is\ not\ set ]]; then
		sym=${BASH_REMATCH[1]}
		if grep -qE "^$sym=" "$config"; then
			echo "kernel config: wanted $sym off, got: $(grep -E "^$sym=" "$config")" >&2
			bad=1
		fi
	fi
done < <(cat "$@")
if [[ $bad -ne 0 ]]; then
	echo "kernel config does not match the fragments" >&2
	exit 1
fi
