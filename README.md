# rusting

A keyboard-first terminal HTTP client. Requests are plain YAML files in a directory
you own, so a collection is just a folder you can diff, review and commit. `rusting`
is a Rust/ratatui rewrite of the Python/Textual `posting`, and it still reads the
`*.posting.yaml` files that tool writes.

## Install

```bash
brew install sg004baa/tap/rusting
```

Or from a checkout:

```bash
cargo install --path crates/cli
```

Prebuilt archives are attached to every GitHub Release for `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `aarch64-unknown-linux-gnu`, `x86_64-unknown-linux-gnu` and
`x86_64-pc-windows-msvc`.

## Usage

```bash
rusting                                  # open the default collection
rusting --collection ./api --env dev.env # explicit collection and environment
rusting locate config                    # print the config file path
rusting locate collection                # print the default collection path
rusting import openapi.yaml              # import an OpenAPI 3.0/3.1 document
rusting import --output ./api spec.json  # ...into a specific directory
```

`--collection/-c` and `--env/-e` are global; `--env` may be repeated. `import`
writes one request file per operation, grouped into directories by the first tag of
each operation, rewrites every URL against `${BASE_URL}`, and drops a `<name>.env`
file next to them holding `BASE_URL` and the auth variables the requests reference.

## Collections

A collection is a directory tree. Every file ending in `.posting.yaml` under it is a
request; other files are ignored, and a request with no `name` takes its file name.

```yaml
name: echo
description: An echo server.
method: POST
url: ${BASE_URL}/posts/:id
path_params:
- name: id
  value: "42"
params:
- name: q
  value: "1"
headers:
- name: X-Setup-Var
  value: $setup_var
  enabled: false          # rows default to enabled: true
body:
  content: '{"a": 1}'     # or: form_data: [{name: something, value: "123"}]
  content_type: application/json
auth:
  type: basic             # basic | digest | bearer_token
  basic:
    username: darren
    password: ${API_PASSWORD}
scripts:
  setup: scripts/hooks.js
  on_request: scripts/hooks.js:prepare
options:
  follow_redirects: true
  verify_ssl: true
  attach_cookies: true
  proxy_url: ""
  timeout: 5.0
```

Fields equal to their default are omitted when rusting writes the file back, and
keys it does not recognise are ignored rather than rejected.

## Environments

Environments are dotenv files. Pass them with `--env`, as many as you like; later
files win over earlier ones. With no `--env`, a file named `rusting.env` in the
current directory is loaded if it exists. The host process environment is *not*
visible to requests unless you set `use_host_environment: true` in the config.

Anywhere in a request you can write `$NAME` or `${NAME}`; `$$` is a literal `$`.
Substitution happens just before sending, and an undefined name is an error rather
than an empty string. Settings themselves can also be overridden through the
environment with `RUSTING_`-prefixed variables, nesting on `__`
(e.g. `RUSTING_SSL__CA_BUNDLE`).

## Scripts

Hooks are ES modules run on an embedded QuickJS engine, resolved relative to the
collection root. `scripts.setup`, `scripts.on_request` and `scripts.on_response`
each name a file, optionally with `:functionName`; the default export name matches
the YAML key. `on_request` receives a mutable request object; `on_response` and
`rusting.variables` are read-only. Every hook gets a `rusting` object with
`setVariable`, `clearVariable`, `clearAllVariables` and `notify`, plus a `console`
whose output lands in the response Scripts tab. A hook is interrupted after 5s.

## Keybindings

Collection-pane keys are fixed; the global bindings and every other setting live
in [docs/config.md](docs/config.md).

| Key | Action |
| --- | --- |
| `j` / `k`, `↓` / `↑` | Move down / up |
| `J` / `K` | Move to the next / previous directory |
| `g` / `G`, `Home` / `End` | First / last row |
| `enter`, `l` | Open the request, or toggle a directory |
| `space`, `r` | Toggle a directory |
| `h` | Collapse the parent directory |
| `d`, `Backspace` | Delete the selected request, with confirmation |
| `D` | Delete without confirmation |
| `y` / `Y` | Duplicate the selected request / duplicate immediately |
| `ctrl+n` | New request in the selected directory |
