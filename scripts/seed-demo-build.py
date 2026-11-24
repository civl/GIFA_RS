"""为 gifa-demo 的 build script 预置 manifest / rc 文件（字节精确）。

为什么需要：DSH 沙箱等环境下，工作区构建出的 exe（含 build script）对文件系统
只读（创建/写入/删除全部 os error 5）。build.rs 的 write_or_reuse 允许复用
**内容完全一致**的预置文件；这里从 build.rs 的模板里逐字节还原内容写进 OUT_DIR。

用法：python scripts/seed-demo-build.py [level]
  level 默认 asInvoker（debug 构建）；release 用 requireAdministrator。
"""
import pathlib
import re
import sys

repo = pathlib.Path(__file__).resolve().parent.parent
build_rs = (repo / "apps" / "gifa-demo" / "build.rs").read_bytes().decode("utf-8")

VERSION = "0.2.6"
APP_NAME = "GIFA 原神跟跑助手"
level = sys.argv[1] if len(sys.argv) > 1 else "asInvoker"

# 找到 OUT_DIR：target/debug/build/gifa-demo/<hash>/out（取修改时间最新的一个）
build_root = repo / "target" / "debug" / "build" / "gifa-demo"
outs = sorted(build_root.glob("*/out"), key=lambda p: p.stat().st_mtime, reverse=True)
if not outs:
    sys.exit("没找到 OUT_DIR（先跑一次 cargo build -p gifa-demo 让它报错留下目录）")
out = outs[0]
print(f"OUT_DIR = {out}")


def template(name: str) -> str:
    """从 build.rs 里抠出 r#"..."# 原始串，保留原换行符。"""
    m = re.search(r'let ' + name + r' = format!\(\s*r#"(.*?)"#\s*,\s*\{?', build_rs, re.S)
    if not m:
        # 兜底：按变量名找 r#" 块
        i = build_rs.index(f"let {name}")
        j = build_rs.index('r#"', i)
        k = build_rs.index('"#', j + 3)
        return build_rs[j + 3 : k]
    return m.group(1)


manifest = template("manifest").replace("{VERSION}", VERSION).replace("{level}", level)

icon = (repo / "apps" / "gifa-demo" / "assets" / "icon.ico").resolve()
icon_s = str(icon).replace("\\", "/")
maj, min_, pat = ("0", "2", "6")
rc = template("rc")
for k, v in {
    "{icon_s}": icon_s,
    "{maj}": maj,
    "{min}": min_,
    "{pat}": pat,
    "{APP_NAME}": APP_NAME,
    "{VERSION}": VERSION,
}.items():
    rc = rc.replace(k, v)

targets = [
    (out / f"gifa-{level}.manifest", manifest),
    (out / "gifa.rc", rc),
]
for path, content in targets:
    path.write_bytes(content.encode("utf-8"))
    print(f"预置 {path}（{len(content.encode('utf-8'))} 字节）")
