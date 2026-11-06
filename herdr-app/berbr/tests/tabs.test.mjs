import test from "node:test";
import assert from "node:assert/strict";
import {registerHooks} from "node:module";
import {state} from "./support/runtime.mjs";

registerHooks({resolve(specifier, context, next) {
  return ["gpui-kit", "gpui-base", "herdr"].includes(specifier)
    ? {url:new URL("./support/runtime.mjs", import.meta.url).href, shortCircuit:true}
    : next(specifier, context);
}});
const {default:Tabs} = await import("../../tabs-berbr/main.js");
const nodes = root => root && typeof root === "object" ? [root, ...root.nodes.flatMap(nodes)] : [];
const byId = (root, id) => nodes(root).find(node => node.attributes.id?.[0] === id);
const text = root => root && typeof root === "object" ? root.nodes.map(text).join(" ") : String(root ?? "");
const cx = {stop_propagation() {}};
function setup() {
  Object.assign(state, {spaces:[], panes:[], calls:[], shared:new Map(), layout:{children:[]}, window:null});
  globalThis.localStorage = {getItem:() => null, setItem() {}};
  const view = new Tabs(); view.init(); return view;
}

test("V12: changing active tabs preserves every close slot and its width", () => {
  const view = setup();
  const tabs = active => ["a", "b"].map(id => ({id, name:`Demo ${id}`, named:true, active:active === id}));
  state.spaces = [{id:"space", active:true, tabs:tabs("a")}];
  const before = view.render();
  state.spaces[0].tabs = tabs("b");
  const after = view.render();
  for (const id of ["a", "b"]) {
    const first = byId(before, `tab-${id}`), second = byId(after, `tab-${id}`);
    assert.equal(first.nodes.length, second.nodes.length);
    for (const root of [before, after]) assert.equal(byId(root, `close-${id}`).attributes.size[0], 12);
  }
  assert.ok(byId(before, "close-b").attributes.invisible);
  byId(before, "close-b").attributes.on_click[0]({}, cx);
  assert.deepEqual(state.calls, []);
});

test("V13: dragging tracks a stable tab ID when earlier tabs close, and disappearing sources are safe", () => {
  for (const removed of ["a", "c"]) {
    const view = setup();
    const tabs = ["a", "b", "c"].map(id => ({id, name:id, named:true}));
    state.spaces = [{id:"space", active:true, tabs}];
    const before = view.render();
    byId(before, "tab-c").handlers["on_mouse_down:left"]({click_count:1}, cx);
    assert.equal(view.pressed, "c");
    state.spaces[0].tabs = tabs.filter(tab => tab.id !== removed);
    const after = view.render();
    assert.doesNotThrow(() => byId(after, "tab-b").handlers["on_mouse_up:left"]({}, cx));
    assert.deepEqual(state.calls, removed === "a" ? [["tab.move", {id:"c", index:1}]] : [["tab.select", {id:"b"}]]);
    assert.equal(view.pressed, null);
  }
});

test("V14: explicit tb-style names survive; only unnamed tabs use the program fallback", () => {
  const view = setup();
  state.panes = [{tab:"named", program:{name:"Codex"}}, {tab:"unnamed", program:{name:"Codex"}}];
  state.spaces = [{id:"space", active:true, tabs:[
    {id:"named", name:"tb42", named:true}, {id:"unnamed", name:"2", named:false},
  ]}];
  const rendered = view.render();
  assert.ok(text(byId(rendered, "tab-named")).includes("tb42"));
  assert.ok(text(byId(rendered, "tab-unnamed")).includes("Codex"));
  assert.ok(!text(byId(rendered, "tab-unnamed")).includes("tb43"));
});
