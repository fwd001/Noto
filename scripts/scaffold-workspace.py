"""一次性生成 workspace 骨架：每个 crate 的 Cargo.toml + 占位 lib.rs/main.rs。

只生成"能编译的空壳"，不写业务逻辑（业务由各 crate 的实现者填入）。
"""
import io, os, json

ROOT = r"D:/code/Notes"

LIBS = {
    "notera-core":       ['serde = { workspace = true }', 'serde_json = { workspace = true }',
                          'thiserror = { workspace = true }', 'uuid = { workspace = true }',
                          'chrono = { workspace = true }', 'sha2 = { workspace = true }'],
    "notera-richtext":   ['notera-core = { workspace = true }', 'serde = { workspace = true }',
                          'serde_json = { workspace = true }', 'thiserror = { workspace = true }'],
    "notera-crypto":     ['notera-core = { workspace = true }', 'notera-richtext = { workspace = true }',
                          'serde = { workspace = true }', 'serde_json = { workspace = true }',
                          'thiserror = { workspace = true }', 'sha2 = { workspace = true }',
                          'aes-gcm-siv = { workspace = true }', 'argon2 = { workspace = true }',
                          'rand = { workspace = true }', 'base64 = { workspace = true }'],
    "notera-store":      ['notera-core = { workspace = true }', 'notera-richtext = { workspace = true }',
                          'notera-crypto = { workspace = true }', 'serde = { workspace = true }',
                          'serde_json = { workspace = true }', 'thiserror = { workspace = true }',
                          'rusqlite = { workspace = true }', 'chrono = { workspace = true }',
                          'uuid = { workspace = true }'],
    "notera-config":     ['notera-core = { workspace = true }', 'notera-store = { workspace = true }',
                          'serde = { workspace = true }', 'serde_json = { workspace = true }',
                          'thiserror = { workspace = true }'],
    "notera-net":        ['notera-core = { workspace = true }', 'notera-config = { workspace = true }',
                          'serde = { workspace = true }', 'serde_json = { workspace = true }',
                          'thiserror = { workspace = true }', 'reqwest = { workspace = true }',
                          'tokio = { workspace = true }', 'bytes = "1"', 'tracing = { workspace = true }',
                          'async-trait = { workspace = true }'],
    "notera-webdav":     ['notera-core = { workspace = true }', 'notera-net = { workspace = true }',
                          'notera-crypto = { workspace = true }', 'serde = { workspace = true }',
                          'serde_json = { workspace = true }', 'thiserror = { workspace = true }',
                          'tokio = { workspace = true }', 'bytes = "1"', 'quick-xml = "0.37"',
                          'async-trait = { workspace = true }', 'tracing = { workspace = true }'],
    "notera-sync":       ['notera-core = { workspace = true }', 'notera-richtext = { workspace = true }',
                          'notera-crypto = { workspace = true }', 'notera-store = { workspace = true }',
                          'notera-webdav = { workspace = true }', 'notera-config = { workspace = true }',
                          'serde = { workspace = true }', 'serde_json = { workspace = true }',
                          'thiserror = { workspace = true }', 'tokio = { workspace = true }',
                          'tracing = { workspace = true }', 'async-trait = { workspace = true }',
                          'futures = "0.3"', 'rand = { workspace = true }'],
    "notera-importer":   ['notera-core = { workspace = true }', 'notera-richtext = { workspace = true }',
                          'notera-crypto = { workspace = true }', 'notera-store = { workspace = true }',
                          'notera-sync = { workspace = true }', 'serde = { workspace = true }',
                          'serde_json = { workspace = true }', 'thiserror = { workspace = true }',
                          'zip = { version = "2", default-features = false, features = ["deflate"] }',
                          'tokio = { workspace = true }'],
    "notera-host":       ['notera-core = { workspace = true }', 'notera-richtext = { workspace = true }',
                          'notera-crypto = { workspace = true }', 'notera-store = { workspace = true }',
                          'notera-sync = { workspace = true }', 'notera-config = { workspace = true }',
                          'notera-importer = { workspace = true }', 'notera-webdav = { workspace = true }',
                          'notera-net = { workspace = true }', 'serde = { workspace = true }',
                          'serde_json = { workspace = true }', 'thiserror = { workspace = true }',
                          'tokio = { workspace = true }', 'tracing = { workspace = true }',
                          'tracing-subscriber = { workspace = true }', 'uuid = { workspace = true }'],
    "notera-test-webdav": ['notera-core = { workspace = true }', 'serde = { workspace = true }',
                           'serde_json = { workspace = true }', 'thiserror = { workspace = true }',
                           'tokio = { workspace = true }', 'bytes = "1"', 'quick-xml = "0.37"',
                           'tracing = { workspace = true }', 'dashmap = { workspace = true }'],
}

BINS = {
    "notera-cli": ['notera-core = { workspace = true }', 'notera-host = { workspace = true }',
                   'notera-store = { workspace = true }', 'notera-sync = { workspace = true }',
                   'notera-webdav = { workspace = true }', 'notera-net = { workspace = true }',
                   'notera-config = { workspace = true }', 'notera-test-webdav = { workspace = true }',
                   'serde = { workspace = true }', 'serde_json = { workspace = true }',
                   'tokio = { workspace = true }', 'clap = { workspace = true }', 'anyhow = { workspace = true }'],
}

DEV_DEPS = ['notera-test-webdav = { workspace = true }', 'tokio = { workspace = true }']

TOML = """[package]
name = "{name}"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
{deps}

[dev-dependencies]
{dev}
"""


def write(path, text):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    if os.path.exists(path):
        return False
    io.open(path, "w", encoding="utf-8", newline="\n").write(text)
    return True


made = 0
for name, deps in list(LIBS.items()):
    p = os.path.join(ROOT, "crates", name, "Cargo.toml")
    if write(p, TOML.format(name=name, deps="\n".join(deps), dev="\n".join(DEV_DEPS))):
        made += 1
    lp = os.path.join(ROOT, "crates", name, "src", "lib.rs")
    write(lp, "//! %s —— 骨架占位，待实现。\n" % name)

for name, deps in BINS.items():
    p = os.path.join(ROOT, "crates", name, "Cargo.toml")
    io.open(os.path.dirname(p), "w", encoding="utf-8").close() if False else os.makedirs(os.path.dirname(p), exist_ok=True)
    if write(p, TOML.format(name=name, deps="\n".join(deps), dev="\n".join(DEV_DEPS))):
        made += 1
    write(os.path.join(ROOT, "crates", name, "src", "main.rs"),
          "//! %s —— 骨架占位，待实现。\nfn main() {}\n" % name)

# 让 bin crate 同时暴露 lib（供集成测试 use）
for name in BINS:
    cp = os.path.join(ROOT, "crates", name, "Cargo.toml")
    s = io.open(cp, encoding="utf-8").read()
    if "[lib]" not in s:
        io.open(cp, "w", encoding="utf-8", newline="\n").write(s + "\n[lib]\nname = \"%s\"\npath = \"src/lib.rs\"\n" % name.replace("-", "_"))
    write(os.path.join(ROOT, "crates", name, "src", "lib.rs"), "//! %s 库面（供测试与 CLI 复用同一实现）。\n" % name)

print("generated Cargo.toml files:", made)
