"""把 build script 的产物预置到**所有**已知的 OUT_DIR（不做哈希猜测）。"""
import pathlib

repo = pathlib.Path.cwd()
build_rs = (repo / "apps" / "gifa-demo" / "build.rs").read_bytes().decode("utf-8")


def template(name: str) -> str:
    i = build_rs.index(f"let {name}")
    j = build_rs.index('r#"', i)
    k = build_rs.index('"#', j + 3)
    return build_rs[j + 3 : k]


manifest = template("manifest").replace("{VERSION}", "0.2.6").replace("{level}", "asInvoker")
icon = str((repo / "apps" / "gifa-demo" / "assets" / "icon.ico").resolve()).replace("\\", "/")
rc = template("rc")
for k, v in {
    "{icon_s}": icon,
    "{maj}": "0",
    "{min}": "2",
    "{pat}": "6",
    "{APP_NAME}": "GIFA 原神跟跑助手",
    "{VERSION}": "0.2.6",
}.items():
    rc = rc.replace(k, v)

n = 0
for out in (repo / "target" / "debug" / "build" / "gifa-demo").glob("*/out"):
    (out / "gifa-asInvoker.manifest").write_bytes(manifest.encode("utf-8"))
    (out / "gifa.rc").write_bytes(rc.encode("utf-8"))
    n += 1
print(f"预置 {n} 个 OUT_DIR")
