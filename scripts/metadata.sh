#!/bin/sh
# Match the board's PATH and credentials; the startup hook itself exits promptly.
exec "${SHELL:-/bin/sh}" -ic 'exec "$HERDR_PLUGIN_ROOT/target/release/berbr" plugin-metadata-start'
