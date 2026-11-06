import test from "node:test";
import assert from "node:assert/strict";
import {readFile, readdir} from "node:fs/promises";

const root = new URL("../", import.meta.url);
const declarations = await readFile(new URL("gpui-kit.d.ts", root), "utf8");
// Generated ambient module blocks close at column zero; their contents are indented.
const exportsByModule = new Map([...declarations.matchAll(/^declare module "([^"]+)" \{([\s\S]*?)^\}/gm)]
  .map(([, name, body]) => [name, new Set([...body.matchAll(/^  export (?:declare )?(?:abstract |async )?(?:function|const|let|var|class|enum)\s+([\w$]+)/gm)]
    .map(match => match[1]))]));

test("named runtime imports in every mod JS file belong to their declared module", async () => {
  assert.ok(exportsByModule.get("gpui-base")?.has("v_virtual_list"));
  assert.equal(exportsByModule.get("gpui-kit")?.has("v_virtual_list"), false);
  for (const directory of [root, new URL("../../tabs-berbr/", import.meta.url)])
  for (const file of (await readdir(directory, {recursive:true})).filter(file => file.endsWith(".js"))) {
    const source = await readFile(new URL(file, directory), "utf8");
    for (const [, names, module] of source.matchAll(/\bimport\s+(?:[\w$]+\s*,\s*)?\{([^}]+)\}\s*from\s*["']([^"']+)["']/g)) {
      if (module.startsWith(".")) continue;
      const exports = exportsByModule.get(module);
      assert.ok(exports, `${file}: no declaration block for ${module}`);
      for (const name of names.split(",").map(name => name.trim().split(/\s+as\s+/)[0]).filter(Boolean))
        assert.ok(exports.has(name), `${file}: ${module} does not export ${name}`);
    }
  }
});

test("interactive mod rows use plain scroll containers, not lazy list renderers", async () => {
  // gpui-kit.d.ts: on_item_click / list prohibit handlers inside item renderers.
  // These rows need independent links, hover controls and prompt choice buttons.
  const lazyLists = new Set(["v_virtual_list", "uniform_list", "list"]);
  for (const directory of [root, new URL("../../tabs-berbr/", import.meta.url)])
  for (const file of (await readdir(directory, {recursive:true})).filter(file => file.endsWith(".js"))) {
    const source = await readFile(new URL(file, directory), "utf8");
    for (const [, names] of source.matchAll(/\bimport\s+(?:[\w$]+\s*,\s*)?\{([^}]+)\}\s*from\s*["']gpui-base["']/g)) {
      for (const name of names.split(",").map(name => name.trim().split(/\s+as\s+/)[0]).filter(Boolean))
        assert.ok(!lazyLists.has(name), `${file}: ${name} cannot host the row handlers in this runtime`);
    }
  }
});
