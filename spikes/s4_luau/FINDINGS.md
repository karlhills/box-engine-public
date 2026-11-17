# Spike S4: Luau binding shape — findings

Question: how should box-engine bind Luau (mlua 0.12.2, Luau 0.740) before M7 builds `box_script`? Run: `cargo run --release` in this folder (prints every number). Machine: Apple M3 Ultra.

## Numbers

| Measurement | Result |
|---|---|
| Move a World into an `Rc<RefCell<World>>` and back (per script stage) | 0.028 µs |
| `lua.scope` creating 40 world functions (per frame) | 6.6 µs, handles invalid after the scope |
| 10,000 `on_update` calls, Rust calls each instance | 0.505 ms (50 ns/call) |
| … Lua dispatcher loop (1 Rust call) | 0.27 ms; with `pcall` per instance 0.56 ms |
| … module in its own environment table | same; but a global lookup (`math.sin`) costs +60% unless the table has `safeenv` (then same as shared globals) |
| 10,000 full-userdata entity handles | 0.94 ms (94 ns each); light userdata: no allocation |
| Typed `get_pos` / `set_pos` (translation) | 0.035 µs each |
| Generic component get, direct reflection walk (Transform) | 0.49 µs; with interned keys 0.45 µs |
| Generic patch set (`{translation = v}`) | 0.31 µs |
| Generic get / set through JSON (`value_to_json`/`value_from_json`) | 2.2 µs / 2.7 µs |
| Batch over 5,000: Rust calls a Lua closure per row | 316 µs |
| … arrays out, Lua loop, arrays back | 499 µs |
| … entity array + typed get/set per entity | 576 µs |
| `Quat` userdata from Rust / table from Rust / Lua table literal | 0.086 / 0.219 / 0.029 µs |
| `vector` arithmetic `(v + v) * 0.5` | 0.005 µs; `quat * vector` userdata 0.062 µs |
| Resume 1,000 waiting coroutines | 38 µs |

## Behaviour

- Errors carry chunk names: `scripts/bad.luau:4: attempt to index nil with 'y'` plus a traceback; syntax errors `scripts/syntax.luau:1: Expected identifier…`.
- Sandbox (`Lua::sandbox(true)`): no `io`, `os.execute`, `os.getenv`, `os.remove`, `package`, `dofile`; built-in tables are read-only. mlua installs a filesystem `require` and `loadstring` exists: both must be replaced/removed. `os.clock` stays.
- Interrupt callback (`set_interrupt`) stops `while true do end` with an error.
- Per-script environment tables keep globals from leaking between scripts.
- Saved state: a Lua table converts to JSON and back through mlua's serde; `vector` becomes `[x, y, z]` (so tag it to round-trip).
- Errors from Rust callbacks reach `pcall` as userdata (use `tostring(err)`).
- Luau whole numbers read back as `Value::Integer` (handle both number variants).

## Decisions (adopted by box_script)

1. The world moves into the VM's host for each script stage (no scope, no pointer).
2. Rust calls each instance's callback (error isolation per instance for free).
3. One environment table per script with `safeenv`.
4. Entities are light userdata with a type metatable.
5. Component values are copies; writes patch type-directed; transform fast paths; JSON only as the fallback for opaque types.
6. `Vec3` = native `vector`, `Quat` = userdata, `Color` = table.
7. Batch: entity arrays from typed per-component queries.
8. Waits are coroutine yields read by a Rust scheduler.
9. Script state for saves: plain tables through `on_save`/`on_load`, vectors/rotations/entities tagged.
