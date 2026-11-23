# SPDX-License-Identifier: MPL-2.0
# Source this (". dev/env.sh") from the repository root: mothrix then keeps
# its settings, sessions and cache under target/dev instead of your own.
export XDG_CONFIG_HOME="$PWD/target/dev/config"
export XDG_DATA_HOME="$PWD/target/dev/data"
export XDG_CACHE_HOME="$PWD/target/dev/cache"
