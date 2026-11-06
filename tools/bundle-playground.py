#!/usr/bin/env python3
"""Bundle web/index.html + web/pkg + examples into one self-contained HTML file."""
import base64, gzip, json, pathlib, re, sys

root = pathlib.Path(__file__).resolve().parent.parent
web = root / "web"
out = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else web / "cera-playground.html"

html = (web / "index.html").read_text()
glue = (web / "pkg" / "cera_wasm.js").read_text()
wasm = gzip.compress((web / "pkg" / "cera_wasm_bg.wasm").read_bytes(), 9)
examples = (web / "examples.json").read_text()

# Turn the ES-module glue into plain inline code.
glue = re.sub(r"^export default (\w+);\s*$", "", glue, flags=re.M)
glue = re.sub(r"^export \{[^}]*\};\s*$", "", glue, flags=re.M)
glue = re.sub(r"^export (async function|function|class|const|let)", r"\1", glue, flags=re.M)
default_init = re.search(r"async function (__wbg_init)\b", glue).group(1)

m = re.search(r'<script type="module">(.*?)</script>', html, re.S)
app = m.group(1)
app = app.replace('import init, { run, check, format } from "./pkg/cera_wasm.js";\n', "")
app = app.replace(
    'try { examples = await (await fetch("./examples.json")).json(); } catch {}',
    "examples = JSON.parse(document.getElementById('examples-data').textContent);",
)
app = app.replace(
    "await init();",
    "const gz = Uint8Array.from(atob(document.getElementById('wasm-data').textContent), c => c.charCodeAt(0));\n"
    "const rawWasm = await new Response(new Blob([gz]).stream().pipeThrough(new DecompressionStream('gzip'))).arrayBuffer();\n"
    f"await {default_init}({{ module_or_path: rawWasm }});",
)

script = f"<script type=\"module\">\n{glue}\nconst {{ run, check, format }} = {{ run, check, format }};\n{app}</script>"
script = script.replace("const { run, check, format } = { run, check, format };\n", "")
bundle = html[: m.start()] + (
    f'<script id="examples-data" type="application/json">{examples.replace("</", "<\\/")}</script>\n'
    f'<script id="wasm-data" type="text/plain">{base64.b64encode(wasm).decode()}</script>\n'
    + script
) + html[m.end():]
out.write_text(bundle)
print(f"{out} ({out.stat().st_size // 1024} KB)")
