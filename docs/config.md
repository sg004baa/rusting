# Configuration

The config file is `config.yaml` in the platform config directory; `rusting locate
config` prints the exact path, and `RUSTING_CONFIG_FILE` overrides it entirely.
Every key is optional.

Settings are layered, highest precedence first: the `RUSTING_*` process
environment, then the `--env` dotenv files in the order given (later files
winning), then this file, then the compiled-in defaults. Both the process
environment and the `--env` files supply settings through `RUSTING_`-prefixed
variables, nesting on `__` — `RUSTING_SSL__CA_BUNDLE` sets `ssl.ca_bundle`.

## Default configuration

```yaml
use_host_environment: false
watch_env_files: true
watch_collection_files: true
auto_save_on_response: false
pager: null        # falls back to $PAGER
pager_json: null   # falls back to pager
editor: null       # falls back to $EDITOR
keymap:
  send-request: "ctrl+j"       # Send request
  focus-method: "ctrl+t"       # Focus method
  focus-url: "ctrl+l"          # Focus URL
  save-request: "ctrl+s"       # Save request
  new-request: "ctrl+n"        # New request
  expand-section: "ctrl+m"     # Expand focused section
  toggle-collection: "ctrl+h"  # Toggle collection browser
  search-requests: "/"         # Search requests
  commands: "ctrl+p"           # Open command palette
  jump: "ctrl+o"               # Jump to a control
  help: "?"                    # Show help
  quit: "ctrl+c"               # Quit rusting
  open-in-pager: "alt+p"       # Open in pager
  open-in-editor: "ctrl+e"     # Open in editor
heading:
  visible: true
  show_host: true
  show_version: true
  hostname: null
url_bar:
  show_value_preview: true
  hide_secrets_in_value_preview: true
response:
  prettify_json: true
  show_size_and_time: true
collection_browser:
  position: left   # left | right
  show_on_startup: true
text_input:
  blinking_cursor: true
focus:
  on_startup: collection   # url | method | collection
  on_response: null        # body | tabs
  on_request_open: null    # headers | body | query | info | url | method | path
ssl:
  ca_bundle: null
  certificate_path: null
  key_file: null
```

## Keymap syntax

A `keymap` value is a comma-separated list of key specs, and it replaces the
default binding for that action rather than adding to it. Each spec is a key
name optionally prefixed by `ctrl+` (or `control+`), `alt+`, `shift+`, `super+`,
`hyper+` or `meta+`, in any order; specs are matched case-insensitively.

The accepted key names are `backspace`, `enter`/`return`, `left`, `right`, `up`,
`down`, `home`, `end`, `pageup`/`page-up`, `pagedown`/`page-down`, `tab`,
`backtab`, `delete`/`del`, `insert`/`ins`, `esc`/`escape`, `space`, `f1`
through `f24`, and any single character such as `/` or `?`.

An unknown action id, an empty key list, or a key spec that does not parse makes
rusting fail to start.
