# Run configuration model (Phase 9.7)

`koren-run` defines language-neutral run requests. A definition is inert: reading,
registering, selecting, or resolving it never starts a process. The desktop host
must pass the final resolved executable, argument vector, working directory,
environment changes, and owner through the existing execution approval service.

The version-one TOML format uses `schema = 1` and one `[[run]]` table per
definition. IDs are unique within a file; the registry scopes them by source
and owner. `executable` and `tool` are mutually exclusive. An empty `languages`
list applies to any language. `output` defaults to `run`, and `save` defaults to
`ask`.

```toml
schema = 1

[[run]]
id = "current-file"
name = "Run current file"
executable = "my-interpreter"
args = ["${file}"]
cwd = "${workspace}"
languages = ["example"]
output = "run"
save = "ask"
```

The supported variables are `${workspace}`, `${file}`, `${fileDir}`,
`${fileName}`, and `${fileStem}`. Expansion happens independently within each
argv element; it never invokes a shell. Missing file context and unknown
variables are errors. The optional readable argument field can be parsed into
argv with quotes and backslash escaping; saved definitions use the explicit
`args` array.

`load_run_file` reports malformed files and unsupported schemas without
executing their contents. `save_run_file` validates all entries, writes a
temporary file beside the target, syncs it, then replaces the target by rename.
The caller is responsible for choosing the user configuration path or the
workspace `.koren/runs.toml` path and for surfacing I/O errors.

The desktop bridge exposes additive protocol-v2 methods to list, save, delete,
select, preview, and request a run configuration. User definitions live in
`runs.toml` under Koren's configuration directory; workspace definitions live
in `.koren/runs.toml`. Selection is saved per workspace in the user
configuration directory. Each one-shot request resolves variables again and
creates a new exact task approval snapshot. A dirty document with `save =
"ask"` requires an explicit `save` or `disk` choice; saving uses the document
service and its conflict checks. Preview and registration never authorize a
process. Interactive terminal output and tool references are reserved by the
model but are not yet launchable through this bridge.
