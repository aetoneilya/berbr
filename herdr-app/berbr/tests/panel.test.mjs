import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { registerHooks } from "node:module";
import { state, theme } from "./support/runtime.mjs";

registerHooks({resolve(specifier, context, next) {
  return ["gpui-kit", "gpui-base", "herdr", "fs/promises"].includes(specifier)
    ? {url:new URL("./support/runtime.mjs", import.meta.url).href, shortCircuit:true}
    : next(specifier, context);
}});
const {default:Berbr} = await import("../main.js");
const {pill, button} = await import("../ui.js");
const snapshot = JSON.parse(await readFile(new URL("../fixtures/sample-snapshot.json", import.meta.url)));
const storage = new Map();
globalThis.localStorage = {getItem:key => storage.get(key) ?? null, setItem:(key, value) => storage.set(key, value)};

function setup({data = snapshot, props = {}, saved = null, collapsed = null, liveSpaces = [], focus = null, keepJobs = false} = {}) {
  storage.clear();
  if (saved) storage.set("berbr.expanded", JSON.stringify(saved));
  if (collapsed) storage.set("berbr.collapsed", JSON.stringify(collapsed));
  Object.assign(state, {focused:focus, props, spaces:liveSpaces, agents:[], panes:[], shared:new Map(), calls:[], overlays:[], notices:[], execs:[], execResult:null,
    existsCalls:[], existsError:null, reads:[], readResult:null, readError:null, watches:[], watchers:[], hcCalls:[], hcResult:null, hcError:null, hcHandler:null, execHandler:null, runHandler:null, files:null, writes:[], writeError:null, writeResult:null});
  const jobs = [], timers = [];
  const handle = {focused:true, focus() {this.focused = true;}, is_focused() {return this.focused;}};
  const cx = {notify() {}, stop_propagation() {}, focus_handle:() => handle,
    timer:{every:(ms, fn) => timers.push({ms, fn})}, sleep:async ms => state.calls.push(["sleep", ms]), spawn:fn => jobs.push(fn)};
  const view = new Berbr();
  view.init({}, cx);
  if (!keepJobs && props.form !== "note") jobs.length = 0; // Skip the initial snapshot and infinite command listener.
  view.loading = false;
  if (!keepJobs && ["done", "remove-merged"].includes(props.form)) view.planLoading = false;
  view.data = data;
  view.selected = data?.rows?.find(row => row.kind === "task")?.id ?? null;
  return {view, cx, jobs, timers, key:key => view.key({key}, cx)};
}
function nodes(root) {
  return root && typeof root === "object" ? [root, ...root.nodes.flatMap(nodes)] : [];
}
const byId = (root, id) => nodes(root).find(node => node.attributes.id?.[0] === id);
const text = root => root && typeof root === "object" ? root.nodes.map(text).join(" ") : String(root ?? "");
const click = (node, cx) => node.attributes.on_click[0]({}, cx);

test("click and Enter on empty, stale, merged, closed and unowned worktrees never launch or remove", () => {
  for (const fields of [{task:"Demo"}, {task:"Demo", merged:true}, {task:"Demo", mr_state:"closed"}, {}]) {
    const tree = {kind:"tree", id:"tree", tree:"Demo", cwd:"/home/demo/worktree", ...fields,
      children:[{kind:"agent", id:"stale", terminal:"gone", state:"working"}]};
    const task = {kind:"task", id:"task", children:[tree]};
    const {view, cx, jobs, key} = setup({data:{rows:[task]}});
    view.showDetails(task, cx);
    const item = view.detailItems(task).find(item => item.type === "tree");
    click(byId(view.render(cx), item.id), cx);
    assert.equal(view.selected, item.id);
    assert.equal(byId(view.render(cx), item.id).attributes.tooltip[0], "No agent here — use Resume");
    key("enter");
    assert.equal(view.selected, item.id);
    view.taskClick(tree, cx);
    view.enter(tree, cx);
    assert.equal(view.selected, tree.id);
    assert.deepEqual(state.calls, []);
    assert.deepEqual(state.execs, []);
    assert.deepEqual(state.hcCalls, []);
    assert.deepEqual(state.overlays, []);
    assert.equal(jobs.length, 0);
    assert.equal(view.expanded, task.id);
  }
});

test("worktree click and Enter focus its newest working live agent, with no CLI or agent launch", () => {
  const tree = {kind:"tree", id:"tree", tree:"Demo", task:"Demo", children:[
    {kind:"agent", id:"idle", terminal:"idle-pane"},
    {kind:"agent", id:"older", terminal:"older-pane"},
    {kind:"agent", id:"newer", terminal:"newer-pane"}]};
  const task = {kind:"task", id:"task", children:[tree]};
  const {view, cx, jobs, key} = setup({data:{rows:[task]}});
  state.agents = [
    {address:"idle-agent", pane:"idle-pane", status:"idle", seq:100},
    {address:"older-agent", pane:"older-pane", status:"working", seq:2},
    {address:"newer-agent", pane:"newer-pane", status:"working", seq:3}];
  view.showDetails(task, cx);
  const item = view.detailItems(task).find(item => item.type === "tree");
  click(byId(view.render(cx), item.id), cx);
  key("enter");
  assert.deepEqual(state.calls, Array.from({length:2}, () => ["agent.focus", {terminal:"newer-agent"}]));
  state.agents = state.agents.map(agent => ({...agent, status:"idle"}));
  key("enter");
  assert.deepEqual(state.calls.at(-1), ["agent.focus", {terminal:"idle-agent"}]);
  assert.deepEqual(state.execs, []);
  assert.deepEqual(state.overlays, []);
  assert.equal(jobs.length, 0);
});

test("unlinked live agents match a worktree by pane cwd and win over shell terminals", () => {
  const tree = {kind:"tree", id:"tree", tree:"Demo", cwd:"/home/demo/worktree/", merged:true};
  const {view, cx, jobs} = setup();
  state.agents = [
    {address:"unrelated", pane:"elsewhere", status:"working", seq:100},
    {address:"agent", pane:"agent-pane", status:"idle", seq:1}];
  state.panes = [
    {terminal:"unrelated", cwd:"/home/demo/worktree-other/src"},
    {terminal:"shell", cwd:"/home/demo/worktree", tab:"shell-tab", focused:true},
    {terminal:"agent", pane:"agent-pane", cwd:"/home/demo/worktree/src", tab:"agent-tab"}];
  view.taskClick(tree, cx);
  assert.deepEqual(state.calls, [["agent.focus", {terminal:"agent"}]]);
  assert.deepEqual(state.execs, []);
  assert.deepEqual(state.overlays, []);
  assert.equal(jobs.length, 0);
});

test("worktree click focuses a live shell tab and terminal, ignoring mod panes and sibling paths", () => {
  const tree = {kind:"tree", id:"tree", tree:"Demo", cwd:"/home/demo/worktree"};
  const {view, cx, jobs} = setup();
  state.panes = [
    {terminal:null, mod:"demo", cwd:tree.cwd, tab:"mod-tab", focused:true},
    {terminal:"unrelated", cwd:tree.cwd + "-other", tab:"wrong-tab", focused:true},
    {terminal:"shell", cwd:tree.cwd + "/src", tab:"shell-tab"}];
  view.taskClick(tree, cx);
  view.enter(tree, cx);
  assert.deepEqual(state.calls, [
    ["tab.select", {id:"shell-tab"}], ["pane.focus", {terminal:"shell"}],
    ["tab.select", {id:"shell-tab"}], ["pane.focus", {terminal:"shell"}]]);
  assert.deepEqual(state.execs, []);
  assert.deepEqual(state.overlays, []);
  assert.equal(jobs.length, 0);
});

test("pills tint theme hex colors, fall back to raised and expose accessible CI status", () => {
  for (const tone of ["blocked", "working", "done", "dim", "accent"]) {
    const rendered = pill({...theme(), [tone]:"#123abc"}, {label:"Status", tone});
    assert.equal(rendered.attributes.bg[0], "#123abc" + (tone === "dim" ? "12" : "24"));
    assert.equal(rendered.attributes.text_color[0], "#123abc");
    assert.equal(rendered.attributes.h[0], 19);
    assert.ok(rendered.attributes.flex_none);
  }
  const rendered = pill(theme(), {label:"CI running", tone:"working", glyph:"spinner", spin:true,
    tooltip:"CI running — open !123 in GitLab"});
  assert.equal(rendered.attributes.bg[0], "raised");
  assert.equal(rendered.attributes.accessibility_label[0], "CI running — open !123 in GitLab");
  assert.equal(rendered.nodes[0].attributes.spin[0], 900);
  assert.ok(rendered.nodes[0].asset.path.endsWith("spinner.svg"));
  assert.ok(pill(theme(), {label:"CI passed", tone:"done", glyph:"check"}).nodes[0].asset.path.endsWith("check.svg"));
});

test("the focus timer follows spaces and terminals only outside panel focus, preserving disclosure state", () => {
  const first = {kind:"task", id:"first", task:"First", children:[{kind:"agent", terminal:"pane-a"}]};
  const second = {kind:"task", id:"second", task:"Second", children:[{kind:"agent", terminal:"pane-b"}]};
  const {view, cx, timers} = setup({data:{rows:[first, second]}, focus:{space:"a", terminal:"pane-a"},
    liveSpaces:[{id:"a", name:"First"}, {id:"b", name:"Second"}]});
  view.expanded = first.id;
  view.collapsed.add("progress");
  view.history.add(first.id);
  view.handle.focused = false;
  const saved = [...storage];
  const tick = () => timers.find(timer => timer.ms === 1000).fn(cx);
  state.focused = {space:"b", terminal:"pane-b"};
  tick();
  assert.equal(view.selected, second.id);
  view.render(cx); // A hidden row must not be replaced with a visible section header.
  assert.equal(view.selected, second.id);
  assert.equal(view.expanded, first.id);
  assert.ok(view.collapsed.has("progress"));
  assert.ok(view.history.has(first.id));
  assert.deepEqual([...storage], saved);
  assert.equal(view.handle.focused, false);
  view.handle.focused = true;
  state.focused = {space:"a", terminal:"pane-a"};
  tick();
  assert.equal(view.selected, second.id);
  view.handle.focused = false;
  view.searchFocused = true;
  state.focused = {space:"b", terminal:"pane-b"};
  tick();
  assert.equal(view.selected, second.id);
  view.searchFocused = false;
  state.focused = {space:"b", terminal:"pane-a"};
  tick();
  assert.equal(view.selected, first.id); // Terminal ownership wins within the same space.
  assert.deepEqual(state.calls, []);
  assert.deepEqual(state.execs, []);
  view.data = {rows:[second]};
  view.render(cx);
  assert.notEqual(view.selected, first.id); // A deleted task cannot remain the followed selection.
});

test("expanded agent and longest matching worktree get a focus marker separate from keyboard selection", () => {
  const parent = {kind:"tree", id:"parent", tree:"parent", cwd:"/home/demo/project"};
  const nested = {kind:"tree", id:"nested", tree:"nested", cwd:"/home/demo/project/nested", children:[
    {kind:"agent", id:"agent", terminal:"pane"}, {kind:"agent", id:"other", terminal:"other"}]};
  const task = {kind:"task", id:"task", children:[parent, nested]};
  const {view, cx, timers} = setup({data:{rows:[task]}, focus:{space:"a", terminal:"address", cwd:nested.cwd + "/src"}});
  state.agents = [{address:"address", pane:"pane", status:"idle"}];
  view.expanded = task.id;
  const marked = root => nodes(root).some(node => node.attributes.accessibility_label?.[0] === "You are here");
  const items = view.detailItems(task);
  for (const item of items.filter(item => ["tree", "agent"].includes(item.type))) {
    view.selected = item.id;
    const row = byId(view.detailRow(theme(), item), item.id);
    assert.equal(row.attributes.bg[0], "selected");
    assert.equal(marked(row), ["nested", "agent"].includes(item.row.id));
  }
  state.focused = {...state.focused, cwd:parent.cwd + "/src", terminal:"other"};
  timers.find(timer => timer.ms === 1000).fn(cx);
  for (const item of items.filter(item => ["tree", "agent"].includes(item.type)))
    assert.equal(marked(view.detailRow(theme(), item)), ["parent", "other"].includes(item.row.id));
});

test("docked CLI and overlays capture live caller env and cwd; explicit overlays retain their invocation", async () => {
  const before = {space:"a", tab:"tab-a", terminal:"pane-a", cwd:"/home/demo/a"};
  const after = {space:"b", tab:"tab-b", terminal:"pane-b", cwd:"/home/demo/b"};
  const {view} = setup({focus:before});
  state.focused = after;
  await view.cli(["select", "Demo"]);
  let [, argv, options] = state.execs.at(-1);
  for (const value of ["HERDR_WORKSPACE_ID=b", "HERDR_TAB_ID=tab-b", "HERDR_PANE_ID=pane-b"]) assert.ok(argv.includes(value));
  assert.ok(!argv.includes("HERDR_TAB_ID=tab-a"));
  assert.deepEqual(options, {cwd:after.cwd});
  view.formOverlay("new");
  assert.deepEqual(state.overlays.at(-1).props.opening, after);
  assert.equal(state.overlays.at(-1).props.cwd, after.cwd);
  const overlay = setup({props:{form:"new", opening:before, cwd:before.cwd}, focus:after});
  await overlay.view.cli(["start", "Demo"]);
  [, argv, options] = state.execs.at(-1);
  assert.ok(argv.includes("HERDR_TAB_ID=tab-a"));
  assert.ok(!argv.includes("HERDR_TAB_ID=tab-b"));
  assert.deepEqual(options, {cwd:before.cwd});
  const dock = setup({focus:before});
  state.focused = {space:"c", tab:"tab-c", terminal:null, cwd:null};
  await dock.view.cli(["app-snapshot"]);
  argv = state.execs.at(-1)[1];
  assert.ok(argv.includes("HERDR_PANE_ID")); // Cleared from the inherited environment.
  assert.ok(!argv.some(value => value.startsWith("HERDR_PANE_ID=")));
  dock.view.project = "demo";
  dock.view.data = {rows:[], projects:[{name:"demo", cwd:"/home/demo/repo"}]};
  await dock.view.cli(["app-snapshot"]);
  assert.deepEqual(state.execs.at(-1)[2], {cwd:"/home/demo/repo"});
});

test("task summary keeps one worst pill and the additional MR count while details include CI", () => {
  const task = {kind:"task", id:"task", children:[
    {kind:"mr", id:"mr", state:"opened", label:"!123 Demo", unresolved:2, ci:"running"},
    {kind:"mr", id:"second-mr", state:"opened", label:"!124 Other", ci:"success"}]};
  const {view} = setup({data:{rows:[task]}});
  const summary = view.taskRow(theme(), task);
  const pills = root => nodes(root).filter(node => node.attributes.accessibility_label?.[0]?.includes(" — open "));
  assert.deepEqual(pills(summary).map(node => node.attributes.tooltip[0]), ["2 threads — open !123 in GitLab"]);
  assert.ok(text(summary).includes("+1"));
  assert.ok(nodes(summary).some(node => node.attributes.truncate));
  assert.deepEqual(pills(view.mrState(theme(), task.children[0])).map(node => node.attributes.tooltip[0]),
    ["2 threads — open !123 in GitLab", "CI running — open !123 in GitLab"]);
});

test("worktree hover actions reserve width at the right edge and keep Review in the menu only", () => {
  const {view} = setup({data:{rows:[]}});
  const mr = {kind:"mr", label:"!123 Demo", state:"merged"};
  const row = {kind:"tree", id:"tree", tree:"Demo", task:"Demo", diff:{adds:8, dels:2}, children:[mr]};
  const item = {id:"detail-tree", type:"tree", row, mr};
  view.selected = null;
  const hidden = byId(view.detailRow(theme(), item), item.id);
  const actions = hidden.nodes.at(-1);
  assert.ok(actions.attributes.invisible);
  assert.deepEqual(actions.nodes.map(node => node.attributes.tooltip[0]), ["Resume agent", "Remove worktree"]);
  assert.ok(hidden.nodes.indexOf(hidden.nodes.find(node => byId(node, `${item.id}/mr`))) < hidden.nodes.indexOf(actions));
  assert.equal(byId(hidden, `${item.id}/review`), undefined);
  view.hovered = item.id;
  const shown = byId(view.detailRow(theme(), item), item.id);
  assert.deepEqual(shown.nodes.map(node => node.attributes.id), hidden.nodes.map(node => node.attributes.id));
  assert.deepEqual(shown.nodes.at(-1).nodes.map(node => node.attributes.size), actions.nodes.map(node => node.attributes.size));
  assert.equal(shown.nodes.at(-1).attributes.invisible, undefined);
  const task = {kind:"task", id:"task", task:"Demo"};
  view.hovered = null;
  const chevron = view.taskRow(theme(), task).nodes.at(-1);
  assert.equal(chevron.attributes.w[0], 22);
  assert.ok(chevron.attributes.invisible);
  view.hovered = task.id;
  assert.equal(view.taskRow(theme(), task).nodes.at(-1).attributes.w[0], 22);
});

test("working task and agent rows show the newest activity, elapsed time and extra agents", () => {
  const task = {id:"activity", kind:"task", title:"Work", children:[
    {id:"first", kind:"agent", terminal:"first-pane"},
    {id:"second", kind:"agent", terminal:"second"},
  ]};
  const {view, cx} = setup({data:{rows:[task]}});
  state.agents = [
    {address:"first", pane:"first-pane", space:"space", tab:"old", status:"working", seq:1,
      title:"Codex — Reading files", app:"codex"},
    {address:"second", space:"space", tab:"new", status:"working", seq:9,
      title:"✳ Claude Code — Fixing tests", app:"claude-code"},
  ];
  state.spaces = [{id:"space", tabs:[{id:"old", working_since:null},
    {id:"new", working_since:Date.now() - 4320000}]}];
  const line = byId(view.render(cx), "task-activity");
  assert.ok(text(line).includes("Fixing tests 1h 12m +1"));
  assert.ok(!text(line).includes("2 working"));
  assert.equal(nodes(line).find(node => text(node) === "1h 12m").attributes.text_color[0], "dim");
  view.showDetails(task, cx);
  const rendered = view.render(cx);
  const items = view.detailItems(task).filter(item => item.type === "agent");
  assert.ok(text(byId(rendered, items[0].id)).includes("Reading files"));
  assert.ok(!text(byId(rendered, items[0].id)).includes("working"));
  assert.ok(text(byId(rendered, items[1].id)).includes("Fixing tests"));
  assert.ok(text(byId(rendered, items[1].id)).includes("1h 12m"));
  state.agents[1].title = "Claude Code — Writing tests";
  state.agents[1].seq++;
  assert.ok(text(view.render(cx)).includes("Writing tests"));
});

test("task and loose agents lead with program images or tinted SVGs and put the spinner on the right", () => {
  for (const [name, path, tint] of [["Codex", "icons/codex.png", false], ["Claude", "icons/claude.svg", true]]) {
    const row = {id:"agent", kind:"agent", terminal:"pane", label:"Snapshot title"};
    const loose = {...row, id:"loose-agent"};
    const task = {id:"task", kind:"task", children:[row]};
    const {view, cx} = setup({data:{rows:[task, loose]}});
    state.agents = [{address:"address", pane:"pane", status:"working", title:`${name} — Fixing tests`,
      app:name.toLowerCase(), program:{name, icon:path, tint}}];
    view.collapsed.clear();
    view.showDetails(task, cx);
    const rendered = view.render(cx);
    const item = view.detailItems(task).find(item => item.type === "agent");
    for (const id of [item.id, "loose-loose-agent"]) {
      const line = byId(rendered, id);
      const mark = line.nodes[0];
      assert.deepEqual(mark.asset, {kind:tint ? "svg" : "image", path});
      assert.equal(mark.attributes.size[0], 14);
      assert.equal(mark.attributes.text_color?.[0], tint ? "muted" : undefined);
      assert.equal(mark.attributes.tooltip[0], name);
      assert.equal(mark.attributes.accessibility_label[0], name);
      const spinner = line.nodes[2].nodes[0];
      assert.ok(spinner.asset.path.endsWith("spinner.svg"));
      assert.equal(spinner.attributes.size[0], 11);
      assert.equal(spinner.attributes.spin[0], 900);
      assert.equal(spinner.attributes.text_color[0], "working");
      assert.ok(text(line).includes("Fixing tests"));
      assert.ok(!text(line).includes(name));
    }
  }
});

test("collapsed activity and expanded agent titles omit their parent worktree suffix", () => {
  const tree = {id:"tree", kind:"tree", tree:"demo-branch", children:[{id:"agent", kind:"agent", terminal:"pane"}]};
  const task = {id:"task", kind:"task", title:"Work", children:[tree]};
  const {view, cx} = setup({data:{rows:[task]}});
  state.agents = [{address:"address", pane:"pane", status:"working", app:"codex", seq:1,
    title:"Codex — Read feed task context | DEMO-BRANCH"}];
  assert.ok(text(byId(view.render(cx), "task-task")).includes("Read feed task context"));
  assert.ok(!text(byId(view.render(cx), "task-task")).includes("| DEMO-BRANCH"));
  view.showDetails(task, cx);
  const agent = view.detailItems(task).find(item => item.type === "agent");
  assert.ok(!text(byId(view.render(cx), agent.id)).includes("| DEMO-BRANCH"));
  state.agents[0].status = "idle";
  state.agents[0].title = "Reviewing changes · demo-branch";
  state.agents[0].seq++;
  assert.ok(text(byId(view.render(cx), agent.id)).includes("Reviewing changes"));
  assert.ok(!text(byId(view.render(cx), agent.id)).includes("· demo-branch"));
});

test("agents without a live program icon fall back to a dim terminal icon", () => {
  const row = {id:"agent", kind:"agent", terminal:"pane", label:"Stopped title",
    program:{name:"Old program", icon:"icons/stale.svg", tint:true}};
  const task = {id:"task", kind:"task", children:[row]};
  const {view, cx} = setup({data:{rows:[task, {...row, id:"loose-agent"}]}});
  view.collapsed.clear();
  view.showDetails(task, cx);
  for (const live of [null, {address:"pane", status:"idle", title:"Live title", program:{name:"Codex", icon:null}}]) {
    state.agents = live ? [live] : [];
    const rendered = view.render(cx);
    const item = view.detailItems(task).find(item => item.type === "agent");
    for (const id of [item.id, "loose-loose-agent"]) {
      const mark = byId(rendered, id).nodes[0];
      assert.deepEqual(mark.asset, {kind:"svg", path:"mods/berbr/icons/terminal.svg"});
      assert.equal(mark.attributes.text_color[0], "dim");
      assert.equal(mark.attributes.tooltip[0], live ? "Codex" : "Terminal");
      assert.equal(mark.attributes.accessibility_label[0], live ? "Codex" : "Terminal");
      assert.ok(!text(byId(rendered, id)).includes("Codex"));
    }
  }
});

test("agent rows always show a status indicator on the right without default text", () => {
  const row = {id:"agent", kind:"agent", terminal:"pane", label:"Snapshot title"};
  const task = {id:"task", kind:"task", children:[row]};
  const {view, cx} = setup({data:{rows:[task, {...row, id:"loose-agent"}]},
    liveSpaces:[{id:"space", tabs:[{id:"tab", working_since:Date.now() - 240000}]}]});
  view.collapsed.clear();
  view.showDetails(task, cx);
  for (const [status, expected, tone] of [
    ["working", "4m", "dim"], ["blocked", "Waiting for you", "blocked"],
    ["done", "finished", "done"], ["idle", "", null], ["unknown", "", null], [null, "stopped", "dim"],
  ]) {
    state.agents = status ? [{address:"pane", status, space:"space", tab:"tab", title:"Live title"}] : [];
    const rendered = view.render(cx);
    const item = view.detailItems(task).find(item => item.type === "agent");
    for (const id of [item.id, "loose-loose-agent"]) {
      const line = byId(rendered, id);
      assert.equal(line.nodes[0].asset.path, "mods/berbr/icons/terminal.svg");
      assert.equal(line.nodes.length, 3);
      const right = line.nodes[2];
      assert.equal(right.attributes.text_size[0], 11);
      assert.equal(right.attributes.text_color[0], tone ?? "dim");
      if (expected) assert.ok(text(right).includes(expected));
      if (["idle", "unknown"].includes(status) || !status) {
        const ring = right.nodes[0].nodes[0];
        assert.equal(ring.attributes.border[0], 1.5);
        assert.equal(ring.attributes.border_color[0], status === "idle" ? "muted" : "dim");
      }
      if (["blocked", "done"].includes(status)) assert.equal(right.nodes[0].nodes[0].attributes.bg[0], status);
      if (status !== "working") assert.ok(!text(right).includes("spinner.svg"));
      assert.ok(!text(line).includes("idle"));
      assert.ok(!text(line).includes("unknown"));
    }
  }
});

test("task status columns keep rings for open tasks and ideas and meaningful active marks", () => {
  const {view} = setup({data:{rows:[]}});
  for (const [status, done, stateName, expected] of [
    [null, false, "active", "ring"], [null, false, "idea", "ring"],
    ["working", false, "active", "spinner"], ["blocked", false, "active", "blocked"],
    ["done", false, "active", "done"], [null, true, "active", "check"],
  ]) {
    const task = {id:"task", kind:"task", title:"Work", done, state:stateName,
      children:[{kind:"agent", terminal:"pane"}]};
    state.agents = status ? [{address:"pane", status}] : [];
    const mark = view.taskRow(theme(), task).nodes[0].nodes[0];
    assert.equal(mark.attributes.w[0], 16);
    assert.equal(mark.attributes.h[0], 16);
    if (expected === "ring") {
      assert.equal(mark.nodes[0].attributes.border[0], 1.5);
      assert.equal(mark.nodes[0].attributes.border_color[0], stateName === "idea" ? "dim" : "muted");
    }
    else if (["blocked", "done"].includes(expected)) assert.equal(mark.nodes[0].attributes.bg[0], expected);
    else assert.ok(text(mark).includes(`${expected}.svg`));
  }
});

test("the existing 30 second refresh timer updates elapsed only for visible working tasks", () => {
  const task = {id:"clock", kind:"task", title:"Clock", children:[{kind:"agent", terminal:"agent"}]};
  const {view, cx, timers} = setup({data:{rows:[task]}});
  assert.deepEqual(timers.map(timer => timer.ms), [1000, 30000, 300000]);
  let refreshed = 0, notified = 0;
  view.refresh = (_cx, cached) => {assert.equal(cached, true); refreshed++;};
  cx.notify = () => notified++;
  const tick = () => timers.find(timer => timer.ms === 30000).fn(cx);
  tick();
  assert.equal(notified, 0);
  state.agents = [{address:"agent", status:"working"}];
  tick();
  assert.equal(notified, 1);
  view.filter = "hidden";
  tick();
  assert.equal(notified, 1);
  view.filter = "";
  state.agents[0].status = "idle";
  tick();
  assert.equal(notified, 1);
  assert.equal(refreshed, 4);
});

test("blocked reasons read once per address and seq, update both rows, and show done review", async () => {
  const task = {id:"blocked", kind:"task", title:"Work", children:[{id:"agent", kind:"agent", terminal:"pane"}]};
  const {view, cx, jobs} = setup({data:{rows:[task]}});
  const agent = {address:"address", pane:"pane", status:"blocked", seq:1, title:"Agent"};
  state.agents = [agent];
  assert.ok(text(byId(view.render(cx), "task-blocked")).includes("Waiting for you"));
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  assert.equal(jobs.length, 1);
  assert.deepEqual(state.hcCalls, []);
  state.hcResult = {code:0, stdout:"$ npm test\nDo you want to proceed?\n1. Yes", stderr:""};
  await jobs.shift()(cx);
  assert.deepEqual(state.hcCalls, [["get", "address"], ["read", "address"]]);
  assert.ok(text(byId(view.render(cx), "task-blocked")).includes("Asks permission · npm test"));
  view.showDetails(task, cx);
  assert.ok(text(view.render(cx)).includes("Asks permission · npm test"));
  assert.equal(jobs.length, 0);
  agent.seq++;
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  state.hcResult.stdout = "Old question?\nWhich branch should I use?\nEnter to send";
  await jobs.shift()(cx);
  assert.ok(text(view.render(cx)).includes("Asks: Which branch should I use?"));
  assert.equal(state.hcCalls.length, 4);
  agent.status = "done"; agent.seq++;
  view.setExpanded(null, cx);
  assert.ok(text(view.render(cx)).includes("Finished · review"));
  view.showDetails(task, cx);
  const item = view.detailItems(task).find(item => item.type === "agent");
  assert.ok(text(byId(view.render(cx), item.id)).includes("finished"));
  assert.equal(jobs.length, 0);
});

test("blocked reads skip hidden and unrelated agents, fail silently and ignore stale results", async () => {
  const task = {id:"blocked", kind:"task", title:"Work", children:[{kind:"agent", terminal:"agent"}]};
  const {view, cx, jobs} = setup({data:{rows:[task]}});
  state.agents = [{address:"agent", status:"blocked", seq:1}, {address:"other", status:"blocked", seq:2}];
  view.filter = "hidden";
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  assert.equal(jobs.length, 0);
  view.filter = "";
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  state.hcError = new Error("Cannot read terminal");
  await jobs.shift()(cx);
  assert.equal(state.hcCalls.length, 2);
  assert.equal(view.error, null);
  assert.ok(text(view.render(cx)).includes("Waiting for you"));
  assert.equal(jobs.length, 0);
  state.hcError = null;
  state.agents[0].seq++;
  state.hcResult = {code:1, stdout:"Allow command?", stderr:"Failed"};
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  await jobs.shift()(cx);
  assert.ok(text(view.render(cx)).includes("Waiting for you"));
  let finish;
  state.agents[0].seq++;
  state.hcResult = new Promise(resolve => {finish = resolve;});
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  const pending = jobs.shift()(cx);
  state.agents[0].status = "working"; state.agents[0].seq++;
  view.syncBlocked(view.liveState().rows, cx); view.render(cx);
  finish({code:0, stdout:"Allow command?", stderr:""});
  await pending;
  assert.equal(view.blockedReasons.size, 0);
  assert.ok(!text(view.render(cx)).includes("Asks permission"));
});

test("close acceptance card is modal, shows the snapshot plan and disables blocked closure", () => {
  const {view, cx} = setup();
  const task = snapshot.rows[0];
  view.showDetails(task, cx);
  click(byId(view.render(cx), `${task.id}/action/close`), cx);
  const spec = state.overlays.at(-1);
  assert.equal(spec.kind, "dialog");
  assert.equal(spec.modal, true);
  assert.equal(spec.width, 520);
  assert.equal(spec.props.row, task);
  const dialog = setup({props:{...spec.props, overlay:"accept"}});
  const rendered = dialog.view.render(dialog.cx);
  for (const label of ["Close “Demo refactor”?", "Blocks closing", "MR !122 is open", "Will be removed",
    "Closes your open tickets: DEMO-45", "Tickets assigned to someone else stay open.",
    "Marks the note done", "6 MRs merged", "Berbr re-checks Jira and GitLab before closing."])
    assert.ok(text(rendered).includes(label), label);
  const confirm = byId(rendered, "done");
  assert.equal(confirm.attributes.tooltip[0], "Resolve the blockers first");
  assert.equal(confirm.attributes.opacity[0], 0.4);
  click(confirm, dialog.cx);
  assert.equal(dialog.jobs.length, 0);
  assert.equal(state.calls.length, 0);
});

test("close acceptance card shows dirty loss, live space, proof link and CLI errors without force", async () => {
  const task = {task:"safe task", title:"Safe task", space:"live", children:[
    {id:"tree", kind:"tree", tree:"checkout", body:"/checkout\n +2"},
    {id:"mr", kind:"mr", label:"!8 Merged work", state:"merged", url:"https://gitlab/mr/8"},
  ]};
  const {view, cx, jobs} = setup({props:{form:"done", task:task.task, label:task.title, row:task, overlay:"accept"},
    liveSpaces:[{id:"live"}]});
  let rendered = view.render(cx);
  assert.ok(text(rendered).includes("checkout — WARNING: deletes 2 uncommitted file(s)"));
  assert.ok(text(rendered).includes("Closes the Herdr space"));
  assert.ok(!text(rendered).includes("Jira Tickets assigned"));
  const dirty = nodes(rendered).find(node => text(node).includes("checkout — WARNING: deletes 2 uncommitted file(s)"));
  assert.ok(dirty);
  click(byId(rendered, "close-proof"), cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs.at(-1), ["open", ["https://gitlab/mr/8"]]);
  const args = [];
  let fail = true;
  view.cli = async values => {args.push(values); if (fail) throw new Error("Jira unavailable"); return "Closed";};
  click(byId(view.render(cx), "done"), cx);
  await jobs.shift()(cx);
  assert.ok(text(view.render(cx)).includes("Jira unavailable"));
  assert.ok(!state.calls.some(([command]) => command === "closeOverlay"));
  fail = false;
  click(byId(view.render(cx), "done"), cx);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["done", "safe task", "--yes"], ["done", "safe task", "--yes"]]);
  assert.ok(state.calls.some(([command, id]) => command === "closeOverlay" && id === "accept"));
});

test("task clicks toggle one persisted expansion; Enter goes to a live space", () => {
  const {view, cx, key} = setup();
  const task = snapshot.rows[0];
  state.spaces = [{id:"live"}];
  const liveTask = {...task, space:"live"};
  view.taskClick(liveTask, cx);
  assert.equal(view.expanded, task.id);
  assert.equal(state.overlays.length, 0);
  assert.equal(state.calls.length, 0);
  assert.equal(JSON.parse(storage.get("berbr.expanded")), task.id);
  view.enter(liveTask, cx);
  assert.deepEqual(state.calls, [["space.select", {id:"live"}]]);
  view.taskClick(liveTask, cx);
  assert.equal(view.expanded, null);
  key("right");
  assert.equal(view.expanded, task.id);
  key("left");
  assert.equal(view.expanded, null);
  key("space");
  assert.equal(view.expanded, task.id);
  const other = snapshot.rows.find(row => row.kind === "task" && row.id !== task.id);
  view.taskClick(other, cx);
  assert.equal(view.expanded, other.id);
  view.taskClick(other, cx);
  key("enter");
  assert.equal(view.expanded, other.id);
});

test("navigation includes inner rows and disclosure contents, excludes ticket chips, and respects focus", () => {
  const {view, cx, key} = setup();
  const task = snapshot.rows[0];
  view.showDetails(task, cx);
  assert.deepEqual(view.rows().slice(2, 10).map(row => row.type), ["tree", "tree", "tree", "tree", "tree", "disclosure", "action", "action"]);
  key("down");
  assert.equal(view.chosen().type, "tree");
  key("left");
  assert.equal(view.selected, task.id);
  const history = view.rows().find(row => row.type === "disclosure");
  view.selected = history.id;
  key("enter");
  assert.equal(view.rows().filter(row => row.historical).length, 4);
  key("j");
  assert.equal(view.chosen().type, "mr");
  key("k");
  assert.equal(view.selected, history.id);
  key("enter");
  assert.equal(view.rows().filter(row => row.historical).length, 0);
  view.handle.focused = false;
  key("down");
  assert.equal(view.selected, history.id);
  view.searchFocused = true;
  key("n");
  assert.equal(state.overlays.length, 0);
  key("up");
  assert.equal(view.chosen().type, "tree");
  view.filter = "search";
  key("escape");
  assert.equal(view.filter, "");
});

test("first snapshot expands the invoking terminal's task, once; saved Done expansion remains visible", async () => {
  const task = {id:"task", kind:"task", task:"work", children:[{kind:"agent", terminal:"caller", task:"work"}]};
  const data = {project:"test", rows:[task]};
  const {view, cx, jobs} = setup({data:null, props:{opening:{terminal:"caller"}}});
  view.cli = async () => JSON.stringify(data);
  view.refresh(cx);
  await jobs.shift()(cx);
  assert.equal(view.expanded, task.id);
  assert.equal(view.selected, task.id);
  view.setExpanded(null, cx);
  view.refresh(cx);
  await jobs.shift()(cx);
  assert.equal(view.expanded, null);
  const done = {...task, done:true};
  const restored = setup({data:null, saved:done.id});
  restored.view.cli = async () => JSON.stringify({project:"test", rows:[done]});
  restored.view.refresh(restored.cx);
  await restored.jobs.shift()(restored.cx);
  assert.equal(restored.view.all, true);
  assert.equal(restored.view.rows()[0].id, "section-done");
  assert.equal(restored.view.rows()[1].id, done.id);
});

test("expanded rendering wires MR and Resume separately and shows live or stopped agents", async () => {
  const {view, cx, jobs} = setup();
  const task = snapshot.rows[0];
  const tree = structuredClone(task.children.find(row => row.tree === "demo-checkout-03"));
  tree.children.push({id:"agent", kind:"agent", label:"Old title", terminal:"pane", state:"done"},
    {id:"stopped", kind:"agent", label:"Stopped agent", terminal:"gone", state:"working"});
  const rich = {...task, children:[tree]};
  state.agents = [{pane:"pane", address:"address", title:"Live title", program:{name:"Codex"}, status:"working"}];
  view.data = {rows:[rich]};
  view.setExpanded(rich.id, cx);
  const item = view.rows().find(row => row.type === "tree");
  view.selected = item.id;
  let rendered = view.render(cx);
  assert.ok(text(rendered).includes("Live title"));
  assert.ok(!text(rendered).includes("Codex ·"));
  assert.ok(text(rendered).includes("stopped"));
  assert.ok(text(rendered).includes("CI running"));
  assert.ok(!text(rendered).includes("Old title"));
  view.cli = async () => {throw new Error("resume failed");};
  assert.equal(byId(rendered, `${item.id}/review`), undefined);
  click(byId(rendered, `${item.id}/resume`), cx);
  assert.equal(view.busy, true);
  rendered = view.render(cx);
  const review = byId(rendered, `${item.id}/resume`);
  assert.ok(text(review).includes("spinner.svg"));
  assert.equal(review.attributes.opacity[0], 1);
  const other = view.rows().find(row => row.type === "agent");
  assert.equal(byId(rendered, other.id).attributes.opacity[0], 0.4);
  await jobs.shift()(cx);
  assert.equal(view.busy, false);
  assert.equal(view.busyAction, null);
  assert.equal(view.error, "resume failed");
  click(byId(view.render(cx), `${item.id}/mr`), cx);
  assert.equal(view.busyAction, `${item.id}/mr`);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs, [["open", [tree.children[0].url]]]);
  assert.equal(state.calls.length, 0);
});

test("inner Enter only focuses existing agents and opens subtask notes", async () => {
  const tree = {id:"tree", kind:"tree", tree:"branch", task:"work", children:[{id:"agent", kind:"agent", terminal:"pane"}]};
  const subtask = {id:"subtask", kind:"task", note:"/notes/subtask.md"};
  const task = {id:"task", kind:"task", task:"work", children:[tree, subtask]};
  const {view, cx, jobs, key} = setup({data:{rows:[task]}});
  state.agents = [{address:"pane", pane:"pane", status:"working", seq:1}];
  view.showDetails(task, cx);
  const args = [];
  view.cli = async values => {args.push(values); return "";};
  key("down");
  key("enter");
  assert.equal(jobs.length, 0);
  assert.deepEqual(args, []);
  assert.deepEqual(state.calls.at(-1), ["agent.focus", {terminal:"pane"}]);
  jobs.length = 0; view.loading = false;
  key("down");
  key("enter");
  assert.deepEqual(state.calls.at(-1), ["agent.focus", {terminal:"pane"}]);
  view.selected = view.rows().find(row => row.type === "disclosure").id;
  key("enter"); key("down"); key("enter");
  assert.equal(state.overlays.at(-1).props.path, subtask.note);
  assert.equal(state.overlays.at(-1).props.form, "note");
  assert.equal(state.overlays.at(-1).key, "berbr-note");
  assert.deepEqual(state.execs, []);
});

test("every fixture task renders expanded with tickets and both disclosures open", () => {
  const {view, cx} = setup();
  view.all = true;
  for (const task of snapshot.rows.filter(row => row.kind === "task")) {
    view.setExpanded(task.id, cx);
    view.history.add(task.id); view.subtasks.add(task.id); view.closedTickets.add(task.id);
    const rendered = view.render(cx);
    assert.ok(byId(rendered, `task-${task.id}`));
    assert.ok(byId(rendered, `${task.id}/add-ticket`));
    for (const item of view.rows().filter(row => row.kind === "detail")) assert.ok(byId(rendered, item.id), item.id);
  }
});

test("main checkout is a label; subtask notes and link failures leave the panel usable", async () => {
  const task = {id:"task", kind:"task", task:"work", children:[
    {id:"agent", kind:"agent", label:"Fallback title", terminal:"terminal"},
    {id:"subtask", kind:"task", note:"/note.md"},
  ]};
  const {view, cx, jobs} = setup({data:{rows:[task]}});
  state.agents = [{address:"terminal", title:"  ", app:"Codex", status:"unknown"}];
  view.showDetails(task, cx);
  const rendered = view.render(cx);
  assert.ok(text(rendered).includes("Main checkout"));
  assert.ok(text(rendered).includes("Fallback title"));
  assert.ok(!text(rendered).includes("Codex ·"));
  assert.equal(byId(rendered, `${task.id}/main/header`).attributes.on_click, undefined);
  state.execResult = {code:1, stdout:"", stderr:"could not open"};
  view.openUrl("/note.md", cx);
  view.taskClick(task, cx);
  assert.equal(view.expanded, task.id);
  await jobs.shift()(cx);
  assert.equal(view.error, "could not open");
  assert.equal(view.busy, false);
  view.taskClick(task, cx);
  assert.equal(view.expanded, null);
});

test("small ticket dialogs retain CLI verbs and show the busy action's spinner", async () => {
  for (const [worktree, verb, id] of [[false, "attach", "attach"], [true, "add", "add"]]) {
    const {view, cx, jobs} = setup({props:{form:"ticket", task:"work"}});
    view.values.ticket = "DEMO-123";
    const args = [];
    view.cli = async values => {args.push(values); return "Done";};
    view.submitTicket(cx, worktree);
    const rendered = view.render(cx);
    assert.ok(text(byId(rendered, id)).includes("spinner.svg"));
    const inactive = byId(rendered, id === "add" ? "attach" : "add");
    assert.equal(inactive.attributes.opacity[0], 0.4);
    click(inactive, cx);
    assert.equal(jobs.length, 1);
    await jobs.shift()(cx);
    assert.deepEqual(args, [worktree ? [verb, "work", "DEMO-123"] : [verb, "DEMO-123", "work"]]);
  }
});


test("loose sections default to active work visible and Cleanup collapsed; old Unsorted state migrates", () => {
  let {view, cx} = setup();
  assert.ok(!view.sections().some(section => section.id === "notInTask"));
  assert.ok(view.sections().find(section => section.id === "progress").rows.some(row => row.kind === "ticket"));
  assert.equal(view.sections().find(section => section.id === "cleanup").expanded, false);
  assert.equal(view.rows().filter(row => row.id?.startsWith("loose:")).length, 6);
  ({view, cx} = setup({collapsed:["inbox", "unsorted"]}));
  assert.deepEqual([...view.collapsed], ["cleanup"]);
  assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), ["cleanup"]);
  view.toggleSection("cleanup", cx);
  assert.equal(view.sections().find(section => section.id === "cleanup").expanded, true);
  assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), []);
});

test("Not in a task gives a second-line reason; Make task prefills both loose and Todo tickets", () => {
  const {view, cx} = setup();
  view.collapsed.clear();
  for (const target of ["DEMO-61", "DEMO-44"]) {
    const row = view.rows().find(row => row.target === target);
    view.taskClick(row, cx);
    const rendered = view.render(cx);
    const action = byId(rendered, `${row.id}/loose/action/make-task`);
    assert.ok(action.nodes.includes("Make task"));
    click(action, cx);
    const props = state.overlays.at(-1).props;
    assert.equal(props.form, "new");
    assert.equal(props.prefill.ticket, target);
    assert.equal(props.prefill.title, row.label.slice(target.length).trim());
    state.props = props;
    const dialog = new Berbr();
    dialog.init({}, cx);
    assert.equal(dialog.source, "jira");
    assert.equal(dialog.ticket, target);
    state.props = {};
    if (target === "DEMO-61") {
      const loose = byId(rendered, `loose-${row.id}`);
      assert.equal(loose.attributes.h[0], 44);
      assert.ok(text(loose).includes("merged · worktree left"));
      assert.equal(loose.nodes[1].nodes[1].attributes.tooltip[0], "merged · worktree left");
    }
  }
  const cleanup = view.rows().find(row => row.target === "DEMO-38");
  const rendered = view.render(cx);
  const row = byId(rendered, `loose-${cleanup.id}`);
  assert.ok(text(row).includes("ticket closed · 2 MRs open"));
  assert.ok(!text(row).includes("Closed"));
  assert.equal(row.attributes.h[0], 44);
  assert.equal(row.nodes.length, 3);
  const reason = row.nodes[1].nodes[1];
  assert.equal(text(reason), "ticket closed · 2 MRs open");
  assert.equal(reason.attributes.tooltip[0], "ticket closed · 2 MRs open");
  assert.equal(reason.attributes.text_size[0], 11);
  assert.equal(reason.attributes.text_color[0], "dim");
  assert.deepEqual(reason.nodes[0].attributes.truncate, []);
  assert.equal(reason.attributes.max_w, undefined);
});

test("expanded details align with the task title, clip exactly two lines and mute completed worktrees", () => {
  const {view, cx} = setup();
  const task = snapshot.rows[0];
  view.showDetails(task, cx);
  const block = view.taskBlock(theme(), task);
  const body = block.nodes[1];
  const titleEdge = 8 + 16 + 10;
  const clip = nodes(body).find(node => node.attributes.max_h?.[0] === 32);
  assert.ok(clip);
  assert.equal(clip.attributes.line_height[0], 1.35);
  assert.equal(clip.attributes.text_size[0], 12);
  assert.equal(clip.attributes.pb, undefined);
  assert.equal(body.attributes.pl[0] + clip.attributes.px[0], titleEdge);
  assert.equal(body.attributes.pl[0] + body.nodes[1].attributes.px[0], titleEdge);
  for (const item of view.detailItems(task)) {
    const row = byId(block, item.id);
    if (!row) continue;
    assert.equal(body.attributes.pl[0] + row.attributes.px[0], titleEdge);
    if (item.type === "tree") {
      const title = row.nodes[1];
      assert.equal(title.attributes.text_color[0], ["merged", "closed"].includes(item.mr?.state) ? "muted" : "foreground");
    }
  }
  const item = {id:"agent", type:"agent", action() {}, row:{label:"Agent"}};
  const agent = view.detailRow(theme(), item);
  assert.equal(agent.attributes.pl, undefined);
  assert.equal(body.attributes.pl[0] + agent.nodes[0].attributes.px[0], titleEdge);
});


test("task-owned worktree clicks only select; Resume keeps explicit select tree arguments", async () => {
  const {view, cx, jobs} = setup();
  const task = snapshot.rows[0];
  view.showDetails(task, cx);
  const item = view.rows().find(row => row.type === "tree" && row.mr?.state === "opened");
  const args = [];
  view.cli = async values => {args.push(values); return "Done";};
  view.handle.focused = false;
  click(byId(view.render(cx), item.id), cx);
  assert.equal(view.selected, item.id);
  assert.equal(view.handle.focused, true);
  assert.equal(jobs.length, 0);
  assert.deepEqual(args, []);
  assert.deepEqual(state.execs, []);
  const rendered = view.render(cx);
  const line = byId(rendered, item.id);
  const ids = nodes(line).map(node => node.attributes?.id?.[0]);
  assert.ok(ids.indexOf(`${item.id}/resume`) > ids.indexOf(`${item.id}/mr`));
  const resume = byId(rendered, `${item.id}/resume`);
  assert.equal(resume.attributes.tooltip[0], "Resume agent");
  assert.equal(resume.attributes.size[0], 20);
  click(resume, cx);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["select", task.task, "--tree", item.row.tree]]);
});

test("merged worktrees only navigate on click and Enter; Resume and Remove remain explicit", async () => {
  const {view, cx, jobs, key} = setup();
  view.showDetails(snapshot.rows[0], cx);
  const item = view.rows().find(row => row.type === "tree" && row.mr?.state === "merged");
  const args = [];
  view.cli = async values => {args.push(values); return "";};
  click(byId(view.render(cx), item.id), cx);
  assert.equal(state.overlays.length, 0);
  assert.equal(jobs.length, 0);
  const rendered = view.render(cx);
  const resume = byId(rendered, `${item.id}/resume`);
  assert.equal(resume.attributes.tooltip[0], "Resume agent");
  click(resume, cx);
  await jobs.shift()(cx);
  jobs.length = 0; view.loading = false;
  key("enter");
  assert.equal(jobs.length, 0);
  assert.deepEqual(args, [["select", item.row.task, "--tree", item.row.tree]]);
  const remove = byId(rendered, `${item.id}/remove`);
  assert.equal(remove.attributes.tooltip[0], "Remove worktree");
  assert.equal(remove.attributes.size[0], 20);
  click(remove, cx);
  assert.equal(state.overlays.length, 1);
  const spec = state.overlays.at(-1);
  assert.equal(spec.props.form, "remove-tree");
  assert.equal(spec.props.tree.tree, item.row.tree);
  assert.equal(spec.width, 440);
  assert.equal(spec.modal, true);
  assert.equal(jobs.length, 0);
});

test("remove confirmation uses an explicit worktree target and closes on success", async () => {
  const tree = {kind:"tree", tree:"branch", cwd:"/home/dev/demo/worktrees/branch", repo:"/home/dev/demo"};
  const mr = {kind:"mr", label:"!42 Title", state:"merged"};
  const {view, cx, jobs} = setup({props:{form:"remove-tree", tree, mr, rows:[], overlay:"confirm"}});
  const args = [];
  view.cli = async values => {args.push(values); return values[0] === "app-snapshot" ? JSON.stringify({rows:[]}) : "Removed";};
  const rendered = view.render(cx);
  assert.ok(text(rendered).includes("Remove worktree “branch”?"));
  assert.ok(text(rendered).includes("Its MR !42 is merged."));
  assert.ok(text(rendered).includes("unknown unpushed commits, or local commits not covered by a merged/closed MR"));
  click(byId(rendered, "remove-tree"), cx);
  assert.equal(view.busy, true);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["done", "--tree", tree.cwd, "--yes", "--repo", tree.repo]]);
  assert.ok(args.every(values => !values.includes("--force")));
  assert.equal(view.busy, false);
  assert.ok(state.calls.some(([command, id]) => command === "closeOverlay" && id === "confirm"));
});

test("task IDs, titles and ticket aliases cannot change a removal target", async () => {
  for (const row of [{task:"branch"}, {task:"demo-0", title:"branch"}, {task:"demo-0", children:[{kind:"ticket", target:"branch"}]}]) {
    const {view, cx, jobs} = setup({props:{form:"remove-tree", tree:{tree:"branch"}, rows:[{kind:"task", ...row}]}});
    const args = [];
    view.cli = async values => {args.push(values); return "Removed";};
    view.submitRemoveTree(cx);
    await jobs.shift()(cx);
    assert.deepEqual(args, [["done", "--tree", "branch", "--yes"]]);
    assert.equal(view.error, null);
  }
});

test("Delete anyway appears only with blockers and is the one path that passes --force", async () => {
  const tree = {kind:"tree", tree:"branch", cwd:"/home/dev/demo/worktrees/branch", body:"/home/dev/demo/worktrees/branch\n +3"};
  let {view, cx, jobs} = setup({props:{form:"remove-tree", tree:{kind:"tree", tree:"clean"}, rows:[]}});
  assert.equal(byId(view.render(cx), "remove-tree-force"), undefined);
  ({view, cx, jobs} = setup({props:{form:"remove-tree", tree, rows:[], overlay:"confirm"}}));
  const args = [];
  view.cli = async values => {args.push(values); return "removed branch";};
  click(byId(view.render(cx), "remove-tree"), cx);
  await jobs.shift()(cx);
  click(byId(view.render(cx), "remove-tree-force"), cx);
  assert.equal(view.busyAction, "remove-tree-force");
  await jobs.shift()(cx);
  assert.deepEqual(args, [["done", "--tree", tree.cwd, "--yes"], ["done", "--tree", tree.cwd, "--yes", "--force"]]);
  ({view, cx, jobs} = setup({props:{form:"remove-tree", tree:{kind:"tree", tree:"clean"}, rows:[]}}));
  view.cli = async () => { throw new Error("\"clean\" is not finished: clean: unpushed commits and no MR"); };
  view.submitRemoveTree(cx);
  await jobs.shift()(cx);
  assert.ok(byId(view.render(cx), "remove-tree-force"));
});

test("remove confirmation displays CLI blockers and retains the form", async () => {
  const {view, cx, jobs} = setup({props:{form:"remove-tree", tree:{tree:"branch"}, rows:[]}});
  view.cli = async () => { throw new Error("Worktree has uncommitted changes"); };
  view.submitRemoveTree(cx);
  await jobs.shift()(cx);
  assert.equal(view.busy, false);
  assert.equal(view.busyAction, null);
  assert.ok(text(view.render(cx)).includes("Worktree has uncommitted changes"));
  assert.equal(state.calls.length, 0);
});

test("Cleanup expands without starting work and offers removal on its inner worktree rows", () => {
  const {view, cx, jobs, key} = setup();
  view.collapsed.delete("cleanup");
  const tree = view.rows().find(row => row.kind === "tree");
  click(byId(view.render(cx), `loose-${tree.id}`), cx);
  assert.equal(view.selected, tree.id);
  assert.equal(view.expanded, null);
  click(byId(view.render(cx), `details-${tree.id}`), cx);
  assert.equal(view.expanded, tree.id);
  assert.equal(jobs.length, 0);
  assert.equal(state.calls.length, 0);
  const inner = view.looseItems(tree).find(item => item.type === "tree");
  view.selected = inner.id;
  let rendered = view.render(cx);
  assert.equal(byId(rendered, `${inner.id}/resume`), undefined);
  click(byId(rendered, `${inner.id}/remove`), cx);
  assert.equal(state.overlays.at(-1).props.tree.tree, tree.tree);
  key("enter");
  assert.equal(state.overlays.length, 1);
  view.selected = tree.id;
  key("enter");
  assert.equal(view.expanded, tree.id);
  click(byId(view.render(cx), `details-${tree.id}`), cx);
  assert.equal(view.expanded, null);
  const ticket = view.rows().find(row => row.target === "DEMO-54");
  view.taskClick(ticket, cx);
  const leftover = view.looseItems(ticket).find(item => item.type === "tree");
  view.selected = leftover.id;
  rendered = view.render(cx);
  click(byId(rendered, `${leftover.id}/remove`), cx);
  assert.equal(state.overlays.at(-1).props.tree.tree, leftover.row.tree);
  assert.equal(jobs.length, 0);
  assert.equal(state.calls.length, 0);
});

test("MR and ticket links have explicit destinations, stop propagation and underline on hover", async () => {
  const {view, cx, jobs} = setup();
  const task = snapshot.rows[0];
  view.showDetails(task, cx);
  const item = view.rows().find(row => row.type === "tree" && row.mr?.state === "opened");
  const link = byId(view.render(cx), `${item.id}/mr`);
  assert.equal(link.attributes.tooltip[0], "Open !122 in GitLab");
  link.attributes.hover[0](link);
  assert.equal(link.attributes.text_color[0], "foreground");
  assert.deepEqual(link.attributes.underline, []);
  let stopped = 0;
  cx.stop_propagation = () => stopped++;
  click(link, cx);
  assert.equal(stopped, 1);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs.at(-1), ["open", [item.mr.url]]);
  assert.equal(view.selected, task.id);
  assert.equal(state.calls.length, 0);
  const ticket = task.children.find(row => row.target === "DEMO-45");
  const chip = byId(view.render(cx), `${task.id}/${ticket.id}`);
  assert.equal(chip.attributes.tooltip[0], "Open DEMO-45 in Jira");
  click(chip, cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs.at(-1), ["open", [ticket.url]]);
  assert.equal(stopped, 2);
});


test("loose worktrees never select their tree name, including children of loose tickets", () => {
  const {view, cx, jobs} = setup();
  const args = [];
  view.cli = async values => {args.push(values); return "";};
  view.collapsed.clear();
  const looseRows = view.sections().filter(section => ["cleanup", "progress"].includes(section.id)).flatMap(section => section.rows)
    .filter(row => row.kind !== "task");
  const trees = looseRows.flatMap(row => row.kind === "tree" ? [row] : (row.children ?? []).filter(child => child.kind === "tree"));
  assert.ok(trees.length > 1);
  for (const tree of trees) {
    view.selected = tree.id;
    const actions = view.treeActions(theme(), tree, tree.children?.find(child => child.kind === "mr"), tree.id)[0].nodes;
    assert.equal(actions.length, 1);
    assert.equal(actions[0].attributes.tooltip[0], "Remove worktree");
    assert.equal(actions[0].attributes.id[0], `${tree.id}/remove`);
    click(actions[0], cx);
    view.open(tree, cx);
    view.enterTree(tree, tree.children?.find(child => child.kind === "mr"), cx, tree.id);
    view.enter(tree, cx);
    view.taskClick(tree, cx);
    assert.equal(state.overlays.at(-1).props.form, "remove-tree");
    assert.equal(state.overlays.at(-1).props.tree.tree, tree.tree);
  }
  assert.equal(jobs.length, 0);
  assert.deepEqual(args, []);
  assert.equal(state.calls.length, 0);
});

test("task-owned trees among loose rows navigate on click and resume only with the button", async () => {
  const tree = {id:"loose-tree", kind:"tree", tree:"loose", task:"old task"};
  const {view, cx, jobs} = setup({data:{rows:[tree]}});
  view.collapsed.clear();
  view.showDetails(tree, cx);
  const inner = view.looseItems(tree).find(item => item.type === "tree");
  view.selected = inner.id;
  const rendered = view.render(cx);
  assert.ok(byId(rendered, `${inner.id}/resume`));
  const args = [];
  view.cli = async values => {args.push(values); return "Done";};
  click(byId(rendered, inner.id), cx);
  assert.equal(jobs.length, 0);
  assert.deepEqual(args, []);
  click(byId(view.render(cx), `${inner.id}/resume`), cx);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["select", "old task", "--tree", "loose"]]);
  assert.equal(state.overlays.length, 0);
});

test("all merged and closed markers keep Resume primary alongside explicit Remove", async () => {
  for (const [fields, mrState] of [[{merged:true}, null], [{mr_state:"merged"}, null],
    [{mr_state:"closed"}, null], [{state:"finished"}, null], [{}, "merged"], [{}, "closed"], [{mr_state:"opened"}, "closed"]]) {
    const mr = mrState ? {kind:"mr", state:mrState} : null;
    const row = {kind:"tree", tree:"demo", task:"task", ...fields};
    const {view, cx, jobs} = setup();
    view.selected = "demo";
    const actions = view.treeActions(theme(), row, mr, "demo")[0].nodes;
    assert.equal(actions[0].attributes.tooltip[0], "Resume agent");
    assert.ok(actions.some(node => node.attributes.tooltip[0] === "Remove worktree"));
    const args = [];
    view.cli = async values => {args.push(values); return "Done";};
    click(actions[0], cx);
    await jobs.shift()(cx);
    assert.deepEqual(args, [["select", "task", "--tree", "demo"]]);
    assert.equal(state.overlays.length, 0);
  }
});

test("CLI errors clean both streams and preserve zsh invocation and stdout on success", async () => {
  const {view} = setup();
  const noise = "(eval):1: can't change option: zle";
  const message = 'Error: no task or ticket "demo-checkout-01"';
  for (const [stderr, stdout, expected] of [
    [noise + noise + message, "", message],
    [noise, noise + message, message],
    [noise + noise, noise, "berbr exited 3"],
    ["  specific error  ", "other error", "specific error"],
  ]) {
    state.execResult = {code:3, stderr, stdout};
    await assert.rejects(view.cli(["select", "task"]), {message:expected});
    const [binary, args] = state.execs.at(-1);
    assert.equal(binary, "zsh");
    assert.deepEqual(args.slice(0, 3), ["-lic", 'exec "$@"', "berbr-env"]);
  }
  state.execResult = {code:0, stderr:noise, stdout:'{"rows":[]}\n'};
  assert.equal(await view.cli(["app-snapshot"]), '{"rows":[]}\n');
});

test("loose ticket click and Enter toggle the shared accordion without opening a URL", () => {
  const {view, cx, jobs, key} = setup();
  view.showDetails(snapshot.rows[0], cx);
  view.collapsed.clear();
  const row = view.rows().find(row => row.target === "DEMO-61");
  view.handle.focused = false;
  click(byId(view.render(cx), `loose-${row.id}`), cx);
  assert.equal(view.expanded, row.id);
  assert.equal(view.selected, row.id);
  assert.equal(view.handle.focused, true);
  assert.equal(JSON.parse(storage.get("berbr.expanded")), row.id);
  assert.deepEqual(state.execs, []);
  assert.deepEqual(state.calls, []);
  assert.equal(jobs.length, 0);
  const rendered = view.render(cx);
  assert.ok(text(rendered).includes("The branch is merged; its worktree is still on disk."));
  assert.ok(text(rendered).includes("Open in Jira"));
  const ticket = view.looseItems(row).find(item => item.type === "ticket");
  assert.equal(byId(rendered, `${ticket.id}/ticket`).attributes.tooltip[0], "Open in Jira");
  key("down");
  assert.equal(view.chosen().type, "ticket");
  key("down");
  assert.equal(view.chosen().type, "tree");
  key("left");
  assert.equal(view.selected, row.id);
  key("enter");
  assert.equal(view.expanded, null);
  key("enter");
  assert.equal(view.expanded, row.id);
  assert.deepEqual(state.execs, []);
});

test("loose worktree Resume selects its ticket; inner Enter never opens removal", async () => {
  const {view, cx, jobs} = setup();
  const row = view.rows().find(row => row.target === "DEMO-61");
  view.taskClick(row, cx);
  const tree = view.looseItems(row).find(item => item.type === "tree");
  view.selected = tree.id;
  let rendered = view.render(cx);
  assert.ok(byId(rendered, `${tree.id}/resume`));
  assert.ok(byId(rendered, `${tree.id}/remove`));
  const args = [];
  view.cli = async values => {args.push(values); return "Done";};
  click(byId(rendered, `${tree.id}/resume`), cx);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["select", "DEMO-61"]]);
  jobs.length = 0; view.loading = false;
  view.enter(view.chosen(), cx);
  assert.equal(state.overlays.length, 0);
  assert.equal(jobs.length, 0);
  jobs.length = 0; view.loading = false;
  assert.ok(args.every(values => values[1] !== tree.row.tree));
  view.collapsed.delete("cleanup");
  const closed = view.rows().find(row => row.target === "DEMO-54");
  view.taskClick(closed, cx);
  const leftover = view.looseItems(closed).find(item => item.type === "tree");
  view.selected = leftover.id;
  rendered = view.render(cx);
  assert.equal(byId(rendered, `${leftover.id}/resume`), undefined);
  assert.ok(byId(rendered, `${leftover.id}/remove`));
  assert.equal(view.looseItems(closed).some(item => item.label === "Make task"), false);
});

test("Attach popup lists open tasks and runs attach KEY task, showing errors and retrying", async () => {
  const {view, cx} = setup();
  const row = view.rows().find(row => row.target === "DEMO-61");
  view.taskClick(row, cx);
  const attach = view.looseItems(row).find(item => item.label === "Attach to task…");
  click(byId(view.render(cx), attach.id), cx);
  const spec = state.overlays.at(-1);
  assert.equal(spec.kind, "popup");
  assert.equal(spec.at, "pointer");
  assert.equal(spec.props.form, "attach-task");
  assert.equal(spec.props.ticket, "DEMO-61");
  assert.ok(spec.props.tasks.length > 1);
  assert.ok(spec.props.tasks.every(task => !task.done));
  const popup = setup({props:{...spec.props, overlay:"attach-popup"}});
  const task = spec.props.tasks[0];
  const id = `attach-task-${task.id}`;
  const args = [];
  let fail = true;
  popup.view.cli = async values => {args.push(values); if (fail) throw new Error("Attach failed"); return "Attached";};
  click(byId(popup.view.render(popup.cx), id), popup.cx);
  assert.ok(text(byId(popup.view.render(popup.cx), id)).includes("spinner.svg"));
  await popup.jobs.shift()(popup.cx);
  assert.ok(text(popup.view.render(popup.cx)).includes("Attach failed"));
  assert.equal(state.calls.length, 0);
  fail = false;
  click(byId(popup.view.render(popup.cx), id), popup.cx);
  await popup.jobs.shift()(popup.cx);
  assert.deepEqual(args, [["attach", "DEMO-61", task.task], ["attach", "DEMO-61", task.task]]);
  assert.ok(state.calls.some(([command, id]) => command === "closeOverlay" && id === "attach-popup"));
});

test("Attach popup handles no available tasks and missing ticket keys", () => {
  let {view, cx, jobs} = setup({props:{form:"attach-task", ticket:"KEY-1", tasks:[]}});
  assert.ok(text(view.render(cx)).includes("No open tasks"));
  assert.equal(jobs.length, 0);
  ({view, cx, jobs} = setup({props:{form:"attach-task", tasks:[{id:"task", task:"work"}]}}));
  click(byId(view.render(cx), "attach-task-task"), cx);
  assert.equal(view.error, "Ticket or task name is unavailable.");
  assert.equal(jobs.length, 0);
});

test("worktree state is visible in task details, collapsed loose reasons and loose details", () => {
  const data = structuredClone(snapshot);
  const task = data.rows[0];
  const tree = task.children.find(row => row.kind === "tree");
  tree.body = "/checkout\n +1 ↑3 ✓ merged quiet 35d";
  const loose = data.rows.find(row => row.target === "DEMO-61");
  loose.children.find(row => row.kind === "tree").body = tree.body;
  const {view, cx} = setup({data});
  view.showDetails(task, cx);
  let rendered = view.render(cx);
  const item = view.detailItems(task).find(item => item.row === tree);
  const line = byId(rendered, item.id);
  for (const [label, tone] of [["↑3", "blocked"], ["merged", "dim"], ["35d", "dim"]]) {
    const part = nodes(line).find(node => text(node) === label);
    assert.ok(part, label);
    assert.equal(part.attributes.text_color[0], tone);
  }
  assert.equal(line.nodes[1].attributes.min_w[0], 0);
  assert.ok(line.nodes[1].attributes.flex_1);
  assert.equal(line.nodes[2].attributes.max_w[0], "35%");
  const dot = nodes(line).find(node => node.attributes.tooltip?.[0] === "1 uncommitted file");
  assert.equal(dot.attributes.size[0], 6);
  assert.equal(dot.attributes.bg[0], "working");
  const marks = nodes(line).find(node => node.attributes.tooltip?.[0].startsWith("3 unpushed commits"));
  assert.equal(marks.attributes.tooltip[0], "3 unpushed commits · quiet 35d");
  assert.ok(!text(line).includes("uncommitted"));
  assert.ok(!text(line).includes("unpushed"));
  const header = byId(rendered, `loose-${loose.id}`);
  assert.ok(text(header).includes("merged · worktree left · 1 uncommitted"));
  assert.ok(text(header).includes("↑3 unpushed"));
  view.showDetails(loose, cx);
  rendered = view.render(cx);
  const checkout = view.looseItems(loose).find(item => item.type === "tree");
  assert.ok(text(byId(rendered, checkout.id)).includes("35d"));
  assert.ok(!text(byId(rendered, checkout.id)).includes("quiet 35d"));
  assert.ok(text(byId(rendered, checkout.id)).includes("dev/demo-checkout-16"));
  assert.equal(view.treeState(theme(), {body:"/clean\n"}), null);
  const clean = view.looseContent(theme(), {kind:"tree", body:"/clean\n"}, "no ticket");
  assert.equal(text(clean.nodes[1]), "no ticket");
  for (const part of nodes(view.treeState(theme(), tree))) {
    for (const attribute of ["hover", "cursor_pointer", "id", "on_click"])
      assert.equal(part.attributes[attribute], undefined);
  }
  assert.equal(text(view.treeState(theme(), {state:"finished"})), "merged");
});

test("all loose fixture rows expand with navigable content and explicit links", () => {
  const {view, cx, jobs} = setup();
  view.collapsed.clear();
  const sections = view.sections();
  for (const section of sections) for (const row of section.rows.filter(row => row.kind !== "task")) {
    if (row.kind === "tree") view.showDetails(row, cx);
    else view.taskClick(row, cx);
    const rendered = view.render(cx);
    const items = view.looseItems(row);
    for (const item of items) assert.ok(byId(rendered, item.id), item.id);
    if (row.kind === "ticket") assert.ok(items.some(item => item.label === "Attach to task…"));
    if (row.kind === "mr") assert.equal(items.filter(item => item.type === "mr").length, 1);
    assert.equal(jobs.length, 0);
    assert.equal(state.execs.length, 0);
  }
});

test("task Note opens a keyed Herdr window without executing open", () => {
  const task = {id:"task", kind:"task", title:"My task", note:"/notes/task.md"};
  const {view, cx, jobs} = setup({data:{rows:[task]}});
  view.showDetails(task, cx);
  click(byId(view.render(cx), "task/action/note"), cx);
  assert.deepEqual(state.overlays, [{mod:"berbr", kind:"window", key:"berbr-note", width:720, height:0.75,
    props:{form:"note", path:task.note, title:task.title, vault:false, tickets:[]}}]);
  view.openNote({note:"/notes/other.md", title:"Other"});
  assert.equal(state.overlays.at(-1).key, "berbr-note");
  assert.equal(state.overlays.at(-1).props.path, "/notes/other.md");
  view.openNote({});
  assert.equal(state.overlays.length, 2);
  assert.deepEqual(state.execs, []);
  assert.equal(jobs.length, 0);
});

test("snapshot previews lead to full note viewing and editing, including machine history", async () => {
  const path = "/home/dev/notes/demo-1.md";
  const task = {id:"task:demo-1", kind:"task", task:"demo-1", title:"Demo", note:path,
    body:"Human criteria.", body_truncated:true};
  const panel = setup({data:{version:3, rows:[task]}});
  panel.view.showDetails(task, panel.cx);
  const rendered = panel.view.render(panel.cx);
  assert.ok(text(rendered).includes("Human criteria.…"));
  assert.ok(nodes(rendered).some(node => node.attributes.tooltip?.[0] === "Task preview · open Note for the full text"));
  panel.view.openNote(task);
  const form = state.overlays.at(-1).props;
  assert.equal(form.path, path);
  assert.equal("body" in form, false);

  const source = `---\ntitle: Demo\n---\nHuman criteria.\n\n${"Long human text. ".repeat(300)}\n\n## Agent notes\nFull machine history.\n\n## Sessions\nFull session log.`;
  const {view, cx, jobs} = setup({props:form});
  state.files = new Map([[path, source]]);
  await jobs.shift()(cx);
  assert.deepEqual(state.reads, [path]);
  assert.ok(text(view.render(cx)).includes("Full machine history."));
  assert.ok(text(view.render(cx)).includes("Full session log."));
  assert.equal(view.note.source, source);
  click(byId(view.render(cx), "note-edit"), cx);
  assert.equal(view.note.editor.value(), source);
  view.note.editor.edit(source + "\nHuman edit.", cx);
  click(byId(view.render(cx), "note-save"), cx);
  await jobs.pop()(cx);
  assert.equal(state.writes.at(-1).expected, source);
  assert.equal(state.files.get(path), source + "\nHuman edit.");
});

test("note window loads selectable markdown and wires title bar controls and Escape", async () => {
  const path = "/notes/My task #1.md";
  const {view, cx, jobs, key} = setup({props:{form:"note", path, title:"My task", overlay:"note-window", vault:true}});
  assert.ok(text(view.render(cx)).includes("Loading note…"));
  state.readResult = "---\ntitle: Hidden\n---\n# Visible\n\n[[Page|Link]]\n![[image.png]]";
  await jobs.shift()(cx);
  assert.deepEqual(state.reads, [path]);
  assert.deepEqual(state.existsCalls, ["/notes/Page.md"]);
  let rendered = view.render(cx);
  const markdown = nodes(rendered).find(node => node.attributes.selectable);
  assert.equal(text(markdown), "Visible");
  assert.ok(text(rendered).includes("[Link](berbr-note:"));
  assert.ok(text(rendered).includes("image.png"));
  assert.equal(markdown.attributes.text_size[0], 24);
  const body = byId(rendered, "note-body");
  assert.ok(body.attributes.overflow_y_scrollbar);
  assert.equal(body.attributes.px[0], 24);
  assert.equal(body.attributes.py[0], 16);
  assert.equal(body.nodes[0].attributes.max_w[0], 680);
  const titlebar = rendered.nodes[0].nodes[0];
  assert.equal(titlebar.attributes.h[0], 40);
  assert.equal(titlebar.nodes[0].attributes.text_size[0], 15);
  assert.ok(titlebar.nodes[0].attributes.truncate);
  let stopped = 0;
  cx.stop_propagation = () => stopped++;
  for (const [id, tooltip] of [["note-obsidian", "Open in Obsidian"], ["note-reload", "Reload"], ["note-close", "Close"]]) {
    const control = byId(rendered, id);
    assert.equal(control.attributes.size[0], 20);
    assert.equal(control.attributes.tooltip[0], tooltip);
    assert.equal(control.attributes.on_mouse_down[0], "left");
    control.attributes.on_mouse_down[1]({}, cx);
  }
  assert.equal(stopped, 3);
  click(byId(rendered, "note-obsidian"), cx);
  // Leave the folder watch pending; this job only opens the explicit Obsidian URI.
  await jobs.pop()(cx);
  assert.deepEqual(state.execs, [["open", ["obsidian://open?path=%2Fnotes%2FMy%20task%20%231.md"]]]);
  state.readResult = "Updated";
  click(byId(rendered, "note-reload"), cx);
  await jobs.pop()(cx);
  rendered = view.render(cx);
  assert.equal(view.note.text, "Updated");
  click(byId(rendered, "note-close"), cx);
  view.busy = true;
  key("escape");
  assert.deepEqual(state.calls, [["closeOverlay", "note-window"], ["closeOverlay", "note-window"]]);
});

test("note states distinguish empty, missing and unreadable content and allow retry", async () => {
  const {view, cx, jobs} = setup({props:{form:"note", path:"/note.md"}});
  jobs.shift(); // Exercise all reads through the Reload control.
  for (const [source, error, expected] of [
    [null, null, "/note.md: File not found"],
    [null, new Error("Permission denied"), "/note.md: Permission denied"],
    ["---\ntitle: Only metadata\n---\n", null, "Empty note"],
    ["Recovered", null, "Recovered"],
  ]) {
    state.readResult = source; state.readError = error;
    click(byId(view.render(cx), "note-reload"), cx);
    await jobs.pop()(cx);
    assert.ok(text(view.render(cx)).includes(expected));
    assert.equal(view.note.loading, false);
  }
});

test("note watch reloads only the file or folder root and stops with its copy", async () => {
  const {view, cx, jobs} = setup({props:{form:"note", path:"/notes/task.md"}});
  state.readResult = "Initial";
  await jobs.shift()(cx);
  const watch = jobs.shift()(cx);
  assert.deepEqual(state.watches, ["/notes"]);
  state.watchers.shift().resolve(["/notes/unrelated.md"]);
  await Promise.resolve();
  assert.equal(jobs.length, 0);
  for (const path of ["/notes/task.md", "/notes"]) {
    state.readResult = path;
    state.watchers.shift().resolve([path]);
    await Promise.resolve();
    assert.equal(jobs.length, 1);
    await jobs.shift()(cx);
    assert.equal(view.note.text, path);
  }
  const watched = state.watches.length;
  cx.notify = () => {throw new Error("copy closed");};
  state.watchers.shift().resolve(["/notes/task.md"]);
  await assert.rejects(watch, {message:"copy closed"});
  assert.equal(state.watches.length, watched);
  assert.equal(jobs.length, 0);
});

test("watch errors keep the note readable and concurrent reads keep the latest result", async () => {
  const {view, cx, jobs} = setup({props:{form:"note", path:"/task.md"}});
  let finish;
  state.readResult = new Promise(resolve => {finish = resolve;});
  const first = jobs.shift()(cx);
  const watch = jobs.shift()(cx);
  state.watchers.shift().reject(new Error("Watch unavailable"));
  await watch;
  state.readResult = "Latest";
  click(byId(view.render(cx), "note-reload"), cx);
  await jobs.shift()(cx);
  finish("Stale");
  await first;
  assert.equal(view.note.text, "Latest");
  const rendered = view.render(cx);
  assert.ok(text(rendered).includes("Latest"));
  assert.ok(text(rendered).includes("Live reload unavailable: Watch unavailable"));
});

const backendPlan = overrides => ({kind:"task", name:"Demo", blockers:[],
  remove:[{tree:"clean", path:"/home/dev/demo/clean", uncommitted:0, unpushed:0, merged:true}],
  keep:[{tree:"dirty", reason:"uncommitted changes"}], close_tickets:["DEMO-12"], leave_tickets:["DEMO-13"],
  note_done:true, close_space:true, steps:["remove worktree clean"], ...overrides});

test("close card waits for backend dry-run and uses its removals, keep list, tickets and blockers", async () => {
  const {view, cx, jobs} = setup({props:{form:"done", task:"Demo", row:{children:[]}}, keepJobs:true});
  const args = [];
  view.cli = async values => {args.push(values); return JSON.stringify(backendPlan({blockers:["MR !7 is open"],
    remove:[...backendPlan().remove, {tree:"dirty-open", path:"/home/dev/demo/dirty-open", uncommitted:3, unpushed:0}]}));};
  let rendered = view.render(cx);
  assert.ok(text(rendered).includes("Checking close plan…"));
  assert.ok(text(rendered).includes("spinner.svg"));
  click(byId(rendered, "done"), cx);
  assert.equal(jobs.length, 1);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["done", "Demo", "--dry-run", "--json"]]);
  rendered = view.render(cx);
  for (const value of ["MR !7 is open", "/home/dev/demo/clean", "dirty — uncommitted changes", "DEMO-12", "DEMO-13"])
    assert.ok(text(rendered).includes(value), value);
  assert.ok(text(rendered).includes("dirty-open · /home/dev/demo/dirty-open — WARNING: deletes 3 uncommitted file(s)"));
  assert.equal(byId(rendered, "done").attributes.opacity[0], 0.4);
  click(byId(rendered, "done"), cx);
  assert.equal(jobs.length, 0);
});

test("unknown dry-run flags quietly fall back; operational errors prevent acceptance", async () => {
  for (const [message, fallback] of [["error: unexpected argument '--dry-run' found", true],
    ["unknown flag: --dry-run", true], ["unknown option --json", true], ["GitLab unavailable", false], ["bad JSON", false]]) {
    const {view, cx, jobs} = setup({props:{form:"done", task:"Demo", row:{children:[]}}, keepJobs:true});
    view.cli = async () => {if (message === "bad JSON") return "not json"; throw new Error(message);};
    await jobs.shift()(cx);
    assert.equal(Boolean(view.error), !fallback);
    assert.equal(view.planLoading, false);
    assert.equal(byId(view.render(cx), "done").attributes.opacity[0], fallback ? 1 : 0.4);
  }
});

test("merged metadata offers explicit worktree removal without an MR child", () => {
  const tree = {id:"tree", kind:"tree", tree:"merged", task:"Demo", mr_state:"merged"};
  const task = {id:"task", kind:"task", task:"Demo", children:[tree]};
  const {view, cx, jobs} = setup({data:{version:2, rows:[task]}});
  view.showDetails(task, cx);
  const item = view.detailItems(task).find(item => item.type === "tree");
  view.selected = item.id;
  const rendered = view.render(cx);
  assert.ok(text(byId(rendered, item.id)).includes("merged"));
  assert.equal(byId(rendered, "section-cleanup"), undefined);
  assert.ok(byId(rendered, `${item.id}/resume`));
  click(byId(rendered, `${item.id}/remove`), cx);
  assert.equal(state.overlays.at(-1).props.form, "remove-tree");
  assert.equal(jobs.length, 0);
});

test("Cleanup offers bulk removal for task-owned trees and confirms backend plans before done --merged", async () => {
  const tree = {id:"tree", kind:"tree", tree:"clean", cwd:"/home/dev/demo/clean", merged:true};
  const {view, cx, jobs} = setup({data:{version:2, rows:[{id:"task", kind:"task", children:[tree, {...tree}]},
    {id:"closed-ticket", kind:"ticket", label:"Old ticket", state:"Closed"}]}});
  const rendered = view.render(cx);
  const header = byId(rendered, "section-cleanup");
  assert.ok(byId(header, "remove-merged"));
  click(byId(header, "remove-merged"), cx);
  assert.equal(jobs.length, 0);
  const spec = state.overlays.at(-1);
  assert.equal(spec.modal, true);
  assert.equal(spec.props.form, "remove-merged");
  assert.equal(spec.props.trees.length, 1);
  const dialog = setup({props:{...spec.props, overlay:"bulk"}, keepJobs:true});
  const args = [];
  dialog.view.cli = async values => {args.push(values); return values.includes("--dry-run")
    ? JSON.stringify(backendPlan({kind:"trees", note_done:false, close_space:false})) : "Removed";};
  let modal = dialog.view.render(dialog.cx);
  assert.ok(text(modal).includes("Checking worktrees…"));
  click(byId(modal, "confirm-remove-merged"), dialog.cx);
  assert.equal(dialog.jobs.length, 1);
  await dialog.jobs.shift()(dialog.cx);
  modal = dialog.view.render(dialog.cx);
  for (const value of ["clean", "/home/dev/demo/clean", "dirty — uncommitted changes", "Remove 1 worktrees"])
    assert.ok(text(modal).includes(value), value);
  click(byId(modal, "confirm-remove-merged"), dialog.cx);
  await dialog.jobs.shift()(dialog.cx);
  assert.deepEqual(args, [["done", "--merged", "--dry-run", "--json"], ["done", "--merged", "--yes"]]);
  assert.ok(state.calls.some(([command, id]) => command === "closeOverlay" && id === "bulk"));
});

test("bulk fallback keeps dirty and unmerged trees, handles no candidates and backend blockers", async () => {
  const trees = [{kind:"tree", tree:"clean", merged:true}, {kind:"tree", tree:"dirty", merged:true, body:"+2"},
    {kind:"tree", tree:"ahead", merged:true, body:"↑3"}, {kind:"tree", tree:"unmerged"}];
  for (const mode of ["old", "blocked", "empty", "error"]) {
    const {view, cx, jobs} = setup({props:{form:"remove-merged", trees}, keepJobs:true});
    view.cli = async () => {
      if (mode === "old") throw new Error("unknown option --dry-run");
      if (mode === "error") throw new Error("Git unavailable");
      return JSON.stringify(backendPlan({kind:"trees", remove:[], blockers:mode === "blocked" ? ["Worktree locked"] : []}));
    };
    await jobs.shift()(cx);
    const rendered = view.render(cx);
    if (mode === "old") {
      assert.equal(view.error, null);
      for (const value of ["dirty — uncommitted changes", "ahead — unpushed commits", "unmerged — not merged"])
        assert.ok(text(rendered).includes(value));
      assert.equal(view.bulkPlan.remove.length, 1);
    } else {
      assert.equal(byId(rendered, "confirm-remove-merged").attributes.opacity[0], 0.4);
      click(byId(rendered, "confirm-remove-merged"), cx);
      assert.equal(jobs.length, 0);
    }
  }
  const {view, cx} = setup({data:{rows:[{kind:"tree", id:"tree", merged:false}]}});
  assert.equal(byId(view.render(cx), "remove-merged"), undefined);
});

async function loadedNote(source, extra = {}) {
  const context = setup({props:{form:"note", path:"/home/dev/notes/Demo.md", title:"Demo", overlay:"note", ...extra}});
  state.files = new Map([["/home/dev/notes/Demo.md", source]]);
  await context.jobs.shift()(context.cx);
  context.jobs.length = 0;
  return context;
}

test("note properties show only human metadata, tickets link externally, and editor holds the whole file", async () => {
  const source = "---\nstate: active\ntags: [demo, example]\ncreated: 2026-01-01\ntickets: [DEMO-12]\nsessions:\n  - machine-session-secret\nagent: machine-agent-secret\n---\n# Heading\n\nRead [docs](https://example.com).\n\n```js\nconst demo = 1;\n```\n![[diagram]]";
  const {view, cx, jobs, key} = await loadedNote(source, {tickets:[{key:"DEMO-12", url:"https://jira.example.com/browse/DEMO-12"}]});
  let rendered = view.render(cx);
  for (const value of ["active", "#demo", "#example", "2026-01-01", "DEMO-12", "Heading", "diagram"])
    assert.ok(text(rendered).includes(value));
  for (const value of ["machine-session-secret", "machine-agent-secret", "![[diagram]]"])
    assert.ok(!text(rendered).includes(value));
  assert.equal(byId(rendered, "note-ticket-DEMO-12").attributes.text_color[0], "accent");
  click(byId(rendered, "note-ticket-DEMO-12"), cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs.at(-1), ["open", ["https://jira.example.com/browse/DEMO-12"]]);
  assert.ok(nodes(rendered).some(node => node.attributes.bg?.[0] === "background" && text(node).includes("const demo")));
  key("e");
  assert.equal(view.note.editing, true);
  assert.equal(view.note.editor.value(), source);
  assert.equal(view.note.editor.options.language, "markdown");
  assert.equal(view.note.editor.options.font_family, "Test Mono");
  view.note.editor.edit(source + "\nEdited", cx);
  rendered = view.render(cx);
  assert.ok(text(rendered).includes("Demo · Unsaved"));
  view.key({key:"s", modifiers:{platform:true}}, cx);
  assert.ok(text(view.render(cx)).includes("spinner.svg"));
  await jobs.shift()(cx);
  assert.deepEqual(state.writes, [{path:view.note.path, value:source + "\nEdited", expected:source}]);
  assert.equal(view.note.dirty, false);
  click(byId(view.render(cx), "note-done"), cx);
  await jobs.shift()(cx);
  assert.equal(view.note.editing, false);
  assert.ok(text(view.render(cx)).includes("Edited"));
});

test("note conflict offers Reload and Overwrite and keeps unrelated write errors visible", async () => {
  const {view, cx, jobs} = await loadedNote("Original");
  click(byId(view.render(cx), "note-edit"), cx);
  view.note.editor.edit("Local", cx);
  state.files.set(view.note.path, "External");
  click(byId(view.render(cx), "note-save"), cx);
  await jobs.shift()(cx);
  assert.equal(view.note.editor.value(), "Local");
  assert.ok(text(view.render(cx)).includes("Changed on disk"));
  assert.equal(state.files.get(view.note.path), "External");
  click(byId(view.render(cx), "note-conflict-reload"), cx);
  await jobs.shift()(cx);
  assert.equal(view.note.editor.value(), "External");
  assert.equal(view.note.dirty, false);
  view.note.editor.edit("New local", cx);
  state.files.set(view.note.path, "External again");
  click(byId(view.render(cx), "note-save"), cx);
  await jobs.shift()(cx);
  click(byId(view.render(cx), "note-overwrite"), cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.writes.at(-1), {path:view.note.path, value:"New local", expected:"External again"});
  assert.equal(state.files.get(view.note.path), "New local");
  assert.equal(view.note.conflict, false);
  view.note.editor.edit("Another edit", cx);
  state.writeError = new Error("Permission denied");
  click(byId(view.render(cx), "note-save"), cx);
  await jobs.shift()(cx);
  assert.ok(text(view.render(cx)).includes("Permission denied"));
  assert.equal(view.note.dirty, true);
});

test("Escape and close ask about unsaved edits; save does not erase edits made during writing", async () => {
  const {view, cx, jobs, key} = await loadedNote("Original");
  key("e");
  view.note.editor.edit("Local", cx);
  key("escape");
  assert.ok(text(view.render(cx)).includes("Unsaved changes"));
  assert.deepEqual(state.calls, []);
  click(byId(view.render(cx), "note-keep-editing"), cx);
  assert.equal(view.note.ask, null);
  click(byId(view.render(cx), "note-close"), cx);
  assert.equal(view.note.ask, "close");
  click(byId(view.render(cx), "note-keep-editing"), cx);
  let finish;
  state.writeResult = new Promise(resolve => {finish = resolve;});
  view.key({key:"s", modifiers:{platform:true}}, cx);
  const pending = jobs.shift()(cx);
  view.note.editor.edit("Newer local", cx);
  finish(); await pending;
  assert.equal(view.note.dirty, true);
  assert.equal(view.note.source, "Local");
  key("escape");
  click(byId(view.render(cx), "note-discard"), cx);
  assert.equal(view.note.editing, false);
  await jobs.shift()(cx);
  key("e"); view.note.editor.edit("Edit again", cx);
  click(byId(view.render(cx), "note-close"), cx);
  click(byId(view.render(cx), "note-discard"), cx);
  assert.deepEqual(state.calls, [["closeOverlay", "note"]]);
});

test("note links navigate existing files in the same window and reject missing or unrelated schemes", async () => {
  const source = "[[Page|Alias]] [[Missing]] ![[drawing]] `![[literal]]`\n\n[Docs](https://example.com)";
  const {view, cx, jobs} = setup({props:{form:"note", path:"/home/dev/notes/Demo.md", title:"Demo"}});
  state.files = new Map([["/home/dev/notes/Demo.md", source], ["/home/dev/notes/Page.md", "# Another note"]]);
  await jobs.shift()(cx); jobs.length = 0;
  let rendered = view.render(cx);
  assert.ok(text(rendered).includes("Missing"));
  assert.ok(!text(rendered).includes("[[Missing]]"));
  assert.ok(text(rendered).includes("`![[literal]]`"));
  const link = nodes(rendered).find(node => node.attributes.on_link_click && text(node).includes("Alias"));
  link.attributes.on_link_click[0]("berbr-note:%2Fhome%2Fdev%2Fnotes%2FMissing.md", cx);
  link.attributes.on_link_click[0]("file:///home/dev/notes/Other.md", cx);
  assert.equal(jobs.length, 0);
  link.attributes.on_link_click[0]("berbr-note:%2Fhome%2Fdev%2Fnotes%2FPage.md", cx);
  await jobs.shift()(cx);
  rendered = view.render(cx);
  assert.equal(view.note.path, "/home/dev/notes/Page.md");
  assert.ok(text(rendered).includes("Another note"));
  assert.equal(state.overlays.length, 0);
  const markdown = nodes(rendered).find(node => node.attributes.on_link_click);
  markdown.attributes.on_link_click[0]("https://example.com", cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs.at(-1), ["open", ["https://example.com"]]);
});

test("view watches never replace an editor buffer and resume on Done", async () => {
  const {view, cx, jobs, key} = setup({props:{form:"note", path:"/home/dev/notes/Demo.md"}});
  state.files = new Map([[view.note.path, "Original"]]);
  await jobs.shift()(cx);
  const watch = jobs.shift()(cx);
  key("e"); view.note.editor.edit("Local", cx);
  state.files.set(view.note.path, "External");
  state.watchers.shift().resolve([view.note.path]);
  await Promise.resolve();
  assert.equal(jobs.length, 0);
  assert.equal(view.note.editor.value(), "Local");
  key("escape"); click(byId(view.render(cx), "note-discard"), cx);
  await jobs.shift()(cx);
  assert.equal(view.note.text, "External");
  state.watchers.shift().reject(new Error("Stop watch"));
  await watch;
});

function sectionPanel() {
  const context = setup({data:{rows:[
    {id:"needs-task", kind:"task", title:"Demo needs", children:[{kind:"agent", terminal:"blocked"}]},
    {id:"running-task", kind:"task", title:"Demo running", children:[{kind:"agent", terminal:"working"}]},
    {id:"progress-task", kind:"task", title:"Demo progress"},
    {id:"idea-task", kind:"task", title:"Demo idea", state:"idea"},
    {id:"done-task", kind:"task", title:"Demo done", done:true},
    {kind:"header", label:"no task", children:[
      {id:"loose-ticket", kind:"ticket", target:"DEMO-101", label:"Demo loose", state:"New"},
      {id:"loose-tree", kind:"tree", tree:"demo-tree", label:"Demo tree"},
    ]},
    {kind:"header", label:"assigned to me in Jira", children:[
      {id:"assigned-ticket", kind:"ticket", target:"DEMO-102", label:"Demo assigned", state:"New"},
    ]},
  ]}});
  state.agents = [{address:"blocked", status:"blocked", seq:1}, {address:"working", status:"working", seq:1}];
  context.view.all = true;
  return context;
}

test("every section header toggles and persists, with the specified initial expansion", () => {
  const {view, cx} = sectionPanel();
  const defaults = new Map([["needs", true], ["running", true], ["progress", true], ["todo", true],
    ["cleanup", false], ["done", true]]);
  assert.equal(view.sections().length, defaults.size);
  for (const [id, expanded] of defaults) {
    let section = view.sections().find(section => section.id === id);
    assert.equal(section.expanded, expanded, id);
    let header = byId(view.render(cx), `section-${id}`);
    assert.ok(text(header).includes(`chevron-${expanded ? "down" : "right"}.svg`), id);
    assert.equal(text(header.nodes.at(-1)), ["todo", "progress"].includes(id) ? "2" : "1");
    view.handle.focused = false;
    click(header, cx);
    assert.equal(view.handle.focused, true);
    assert.equal(view.selected, `section-${id}`);
    assert.equal(view.sections().find(section => section.id === id).expanded, !expanded);
    assert.equal(JSON.parse(storage.get("berbr.collapsed")).includes(id), expanded);
    header = byId(view.render(cx), `section-${id}`);
    assert.equal(header.attributes.bg[0], "selected");
    assert.ok(text(header).includes(`chevron-${expanded ? "right" : "down"}.svg`));
    click(header, cx);
    assert.equal(view.sections().find(section => section.id === id).expanded, expanded);
  }
  view.all = false;
  assert.equal(byId(view.render(cx), "section-done"), undefined);
  view.all = true;
  assert.equal(view.sections().find(section => section.id === "done").expanded, true);
});

test("stored section ids migrate together, preserving every new id and removing unknowns", () => {
  const saved = ["needs", "running", "progress", "ideas", "done", "notInTask", "unsorted", "inbox", "unknown", "needs"];
  const expected = ["needs", "running", "progress", "todo", "done", "cleanup"];
  let {view} = setup({collapsed:saved});
  assert.deepEqual([...view.collapsed], expected);
  assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), expected);
  ({view} = setup({collapsed:expected}));
  assert.deepEqual([...view.collapsed], expected);
  ({view} = setup({collapsed:[]}));
  assert.deepEqual([...view.collapsed], []);
  ({view} = setup({collapsed:["inbox", "cleanup"]}));
  assert.deepEqual([...view.collapsed], ["cleanup"]);
  for (const [saved, expected] of [
    [["ideas"], ["todo"]], [["inbox"], []], [["todo", "ideas", "inbox"], ["todo"]],
  ]) {
    ({view} = setup({collapsed:saved}));
    assert.deepEqual([...view.collapsed], expected);
    assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), expected);
  }
});

test("Todo renders idea rings and assigned ticket icons together; hover Make task prefills the dialog", () => {
  const {view, cx} = sectionPanel();
  let rendered = view.render(cx);
  const list = byId(rendered, "berbr-tasks");
  const header = byId(list, "section-todo");
  const index = list.nodes.indexOf(header);
  assert.equal(text(header.nodes.at(-2)), "Todo");
  assert.equal(text(header.nodes.at(-1)), "2");
  assert.equal(list.nodes[index + 1].attributes.id[0], "task-idea-task");
  assert.equal(list.nodes[index + 2].attributes.id[0], "loose-assigned-ticket");
  assert.equal(list.nodes[index + 3].attributes.id[0], "section-cleanup");
  const idea = byId(list, "task-idea-task");
  assert.equal(idea.nodes[0].nodes[0].nodes[0].attributes.border[0], 1.5);
  const ticket = byId(list, "loose-assigned-ticket");
  assert.ok(ticket.nodes[0].asset.path.endsWith("file-text.svg"));
  assert.ok(text(ticket).includes("Assigned in Jira"));
  assert.ok(ticket.nodes.find(node => byId(node, "make-task-assigned-ticket")).attributes.invisible);
  ticket.attributes.on_hover[0](true, cx);
  rendered = view.render(cx);
  click(byId(rendered, "make-task-assigned-ticket"), cx);
  const props = state.overlays.at(-1).props;
  assert.equal(props.form, "new");
  assert.deepEqual(props.prefill, {title:"Demo assigned", ticket:"DEMO-102"});
  const dialog = setup({props}).view;
  assert.equal(dialog.source, "jira");
  assert.equal(dialog.ticket, "DEMO-102");
  assert.equal(dialog.inputs.title.value(), "Demo assigned");
});

test("open count includes ideas and excludes assigned tickets regardless of collapse or filter", () => {
  const idea = {id:"idea", kind:"task", title:"Demo idea", state:"idea"};
  const assigned = {kind:"header", label:"assigned to me in Jira", children:[
    {id:"ticket", kind:"ticket", target:"DEMO-1", label:"Demo ticket"},
  ]};
  const done = {id:"done", kind:"task", title:"Demo done", done:true};
  for (const collapsed of [[], ["todo"]]) {
    const {view, cx} = setup({data:{rows:[idea, assigned, done]}, collapsed});
    assert.ok(!text(view.render(cx)).includes("No open tasks"));
    assert.equal(text(byId(view.render(cx), "toggle-done")), "Done 1");
    view.data = {rows:[assigned, done]};
    assert.ok(text(view.render(cx)).includes("No open tasks"));
    view.filter = "Demo";
    assert.ok(!text(view.render(cx)).includes("No open tasks"));
    assert.equal(text(byId(view.render(cx), "toggle-done")), "Done 1");
  }
});

test("Todo keyboard navigation, filtering and jumps reveal both kinds of rows", () => {
  const {view, cx, key} = sectionPanel();
  view.project = "demo";
  view.collapsed.add("todo");
  view.selected = "section-todo";
  key("right");
  key("down");
  assert.equal(view.chosen().id, "idea-task");
  key("down");
  assert.equal(view.chosen().id, "assigned-ticket");
  view.collapsed.add("todo");
  for (const query of ["Demo idea", "DEMO-102"]) {
    view.filter = query;
    assert.equal(view.sections()[0].id, "todo");
    assert.equal(view.sections()[0].expanded, true);
    assert.equal(view.sections()[0].rows.length, 1);
    assert.ok(view.collapsed.has("todo"));
  }
  for (const [row, kind] of [["idea-task", "task"], ["assigned-ticket", "ticket"]]) {
    view.collapsed.add("todo");
    state.shared.set("berbr.jumpTarget", {project:"demo", row, kind});
    view.receiveJump(cx);
    assert.equal(view.collapsed.has("todo"), false);
    assert.equal(view.filter, "");
    assert.equal(view.expanded, row);
    assert.equal(view.selected, row);
    assert.ok(byId(view.render(cx), `${kind === "task" ? "task" : "loose"}-${row}`));
  }
});

test("arrows stop on collapsed headers; Enter toggles and Right or Left set expansion", () => {
  const {view, cx, key} = sectionPanel();
  const ids = view.sections().map(section => section.id);
  view.collapsed = new Set(ids);
  view.selected = null;
  view.render(cx);
  assert.deepEqual(view.rows().map(row => row.id), ids.map(id => `section-${id}`));
  assert.equal(view.selected, "section-needs");
  for (const id of ids.slice(1)) {key("down"); assert.equal(view.chosen().id, `section-${id}`);}
  key("down");
  assert.equal(view.chosen().id, "section-done");
  key("up");
  assert.equal(view.chosen().id, "section-cleanup");
  key("enter");
  assert.equal(view.collapsed.has("cleanup"), false);
  key("down");
  assert.equal(view.chosen().id, "loose-tree");
  key("up");
  key("left"); key("left");
  assert.equal(view.collapsed.has("cleanup"), true);
  key("right"); key("right");
  assert.equal(view.collapsed.has("cleanup"), false);
  assert.equal(view.selected, "section-cleanup");
  key("enter");
  assert.equal(view.collapsed.has("cleanup"), true);
  assert.equal(view.expanded, null);
  assert.deepEqual(state.calls, []);
  assert.deepEqual(state.execs, []);
});

test("collapsing a task's section hides its details and restores the persisted task expansion", () => {
  const {view, cx} = sectionPanel();
  const task = view.sections().find(section => section.id === "progress").rows[0];
  view.showDetails(task, cx);
  assert.ok(byId(view.render(cx), `${task.id}/action/close`));
  click(byId(view.render(cx), "section-progress"), cx);
  assert.equal(view.expanded, task.id);
  assert.equal(JSON.parse(storage.get("berbr.expanded")), task.id);
  assert.equal(view.selected, "section-progress");
  assert.equal(view.rows().some(row => row.id === task.id || row.parent?.id === task.id), false);
  assert.equal(byId(view.render(cx), `task-${task.id}`), undefined);
  click(byId(view.render(cx), "section-progress"), cx);
  assert.ok(byId(view.render(cx), `${task.id}/action/close`));
  assert.equal(view.expanded, task.id);
});

test("filtering expands all matching sections while retaining collapsed choices and attention counts", () => {
  const {view, cx} = sectionPanel();
  view.collapsed = new Set(view.sections().map(section => section.id));
  const before = [...view.collapsed];
  let rendered = view.render(cx);
  let needs = byId(rendered, "section-needs");
  assert.equal(needs.nodes.at(-1).attributes.text_color[0], "blocked");
  assert.equal(text(needs.nodes.at(-1)), "1");
  assert.equal(byId(rendered, "section-running").nodes.at(-1).attributes.text_color[0], "dim");
  view.filter = "demo";
  rendered = view.render(cx);
  assert.ok(view.sections().every(section => section.expanded));
  assert.deepEqual([...view.collapsed], before);
  for (const id of before) assert.ok(text(byId(rendered, `section-${id}`)).includes("chevron-down.svg"));
  needs = byId(rendered, "section-needs");
  assert.equal(needs.nodes.at(-1).attributes.text_color[0], "dim");
  assert.ok(byId(rendered, "task-needs-task"));
  view.filter = "";
  rendered = view.render(cx);
  assert.ok(view.sections().every(section => !section.expanded));
  assert.equal(byId(rendered, "task-needs-task"), undefined);
  assert.equal(byId(rendered, "section-needs").nodes.at(-1).attributes.text_color[0], "blocked");
});

test("the 20px monitor button is the only Go to space control and stops propagation", () => {
  const task = {id:"space-task", kind:"task", task:"Demo", space:"live", title:"Demo", note:"/notes/demo.md"};
  const {view, cx, jobs} = setup({data:{rows:[task]}, liveSpaces:[{id:"live"}]});
  const actions = view.detailItems(task).filter(item => item.type === "action").map(item => item.label);
  assert.deepEqual(actions, ["Note", "Close task"]);
  const control = byId(view.render(cx), `task-space-${task.id}`);
  assert.equal(control.attributes.size[0], 20);
  assert.equal(control.attributes.tooltip[0], "Go to space");
  assert.equal(control.attributes.accessibility_label[0], "Go to space");
  assert.ok(text(control).includes("monitor.svg"));
  let stopped = 0;
  cx.stop_propagation = () => stopped++;
  click(control, cx);
  assert.equal(stopped, 1);
  assert.equal(view.expanded, null);
  assert.equal(jobs.length, 0);
  assert.deepEqual(state.calls, [["space.select", {id:"live"}]]);
  view.busy = true;
  click(byId(view.render(cx), `task-space-${task.id}`), cx);
  assert.equal(state.calls.length, 1);
  view.busy = false; state.spaces = [];
  assert.equal(byId(view.render(cx), `task-space-${task.id}`), undefined);
});

test("worktree terminal status appears once after the MR key, retaining other Git marks", () => {
  const {view} = setup({data:{rows:[]}});
  for (const [fields, mrState, expected] of [
    [{merged:true}, "merged", "merged"], [{}, "merged", "merged"],
    [{merged:true}, "opened", "merged"], [{mr_state:"merged"}, null, "merged"],
    [{}, "closed", "closed"], [{mr_state:"closed"}, "opened", "closed"],
  ]) {
    const mr = {kind:"mr", label:"!42 Demo change", state:mrState, url:"https://git.example.com/demo/merge_requests/42"};
    const row = {kind:"tree", tree:"demo-checkout", task:"Demo", body:"/home/dev/demo\n +2 ↑3 quiet 9d", children:[mr], ...fields};
    const line = view.detailRow(theme(), {id:"tree-detail", type:"tree", row, mr});
    const label = text(line);
    assert.equal((label.match(new RegExp(`\\b${expected}\\b`, "g")) ?? []).length, 1, label);
    assert.ok(label.indexOf("!42") < label.indexOf(expected), label);
    for (const mark of ["↑3", "9d"]) assert.ok(label.includes(mark));
    const marks = nodes(line).find(node => node.attributes.tooltip?.[0].startsWith("2 uncommitted"));
    assert.ok(!marks.attributes.tooltip[0].includes(expected));
  }
  const row = {kind:"tree", tree:"demo-checkout", merged:true};
  assert.ok(text(view.detailRow(theme(), {id:"no-mr", type:"tree", row})).includes("merged"));
});

test("working task summaries contain only activity, elapsed and extra agent count; signals remain in details", () => {
  for (const blocked of [false, true]) {
    const mr = {id:"mr", kind:"mr", label:"!42 Demo change", state:"opened", body:"ci✗", url:"https://git.example.com/demo/merge_requests/42"};
    const task = {id:"working-task", kind:"task", task:"Demo", title:"Demo task", children:[
      {id:"ticket", kind:"ticket", target:"DEMO-12", label:"DEMO-12 Demo ticket", state:"In review", url:"https://jira.example.com/browse/DEMO-12"},
      {id:"tree", kind:"tree", tree:"demo-checkout", task:"Demo", children:[mr, {id:"agent", kind:"agent", terminal:"first"}]},
      {id:"second-agent", kind:"agent", terminal:"second"},
      ...(blocked ? [{id:"blocked-agent", kind:"agent", terminal:"blocked"}] : []),
    ]};
    const {view, cx} = setup({data:{rows:[task]}, liveSpaces:[{id:"live", tabs:[{id:"tab", working_since:Date.now() - 120000}]}]});
    state.agents = [{address:"first", status:"working", seq:1, title:"Codex — Reading files"},
      {address:"second", status:"working", seq:9, title:"Codex — Reviewing demo", app:"codex", space:"live", tab:"tab"},
      ...(blocked ? [{address:"blocked", status:"blocked", seq:10}] : [])];
    let rendered = view.render(cx);
    const summary = text(byId(rendered, `task-${task.id}`));
    assert.ok(summary.includes("Reviewing demo 2m +1"), summary);
    for (const signal of ["DEMO-12", "in review", "!42", "CI failed", "Waiting for you"])
      assert.ok(!summary.includes(signal), signal);
    assert.equal(byId(rendered, `task-mr-${task.id}`), undefined);
    view.showDetails(task, cx);
    rendered = view.render(cx);
    for (const signal of ["DEMO-12", "In review", "!42", "CI failed"])
      assert.ok(text(rendered).includes(signal), signal);
  }
});

async function waitingPanel({screen, status} = {}) {
  const task = {id:"waiting-task", kind:"task", title:"Demo", children:[
    {id:"tree", kind:"tree", tree:"demo", children:[{id:"agent", kind:"agent", terminal:"terminal"}]}]};
  const panel = setup({data:{project:"demo", rows:[task]}});
  panel.view.project = "demo";
  state.agents = [{address:"address", pane:"terminal", status:"blocked", seq:1, title:"Choose a colour"}];
  const promptStatus = status ?? {state:"blocked", kind:"question", msg:"answer: Which colour?"};
  const promptScreen = screen ?? await readFile(new URL("fixtures/numbered-permission.txt", import.meta.url), "utf8");
  state.hcHandler = async args => ({code:0, stderr:"", stdout:args[0] === "get" ? JSON.stringify({status:promptStatus}) : args[0] === "read" ? promptScreen : ""});
  panel.timers.find(timer => timer.ms === 1000).fn(panel.cx);
  await panel.jobs.shift()(panel.cx);
  return {...panel, task, status:promptStatus, card:panel.view.waitingItems(task)[0]};
}

test("waiting cards render quiet prompts; keyboard 2 sends hc keys once and retains unanswered prompts", async () => {
  const {view, cx, jobs, key, card} = await waitingPanel();
  const rendered = view.render(cx), prompt = byId(rendered, card.id);
  assert.ok(text(prompt).includes("Which colour?"));
  assert.ok(!text(prompt).includes("answer:"));
  assert.ok(text(prompt).includes("2 Yes, and don't ask again"));
  assert.ok(view.rows().some(row => row.id === card.id));
  const question = nodes(prompt).find(node => node.attributes.max_h?.[0] === 49);
  assert.equal(question.attributes.text_color[0], "foreground");
  assert.equal(question.attributes.text_size[0], 12);
  view.selected = card.id;
  state.hcCalls = [];
  key("2");
  key("2");
  assert.equal(jobs.length, 1);
  click(byId(view.render(cx), `${card.id}/1`), cx);
  assert.equal(jobs.length, 1);
  await jobs.shift()(cx);
  assert.deepEqual(state.hcCalls, [["keys", "address", "2"], ["get", "address"]]);
  assert.ok(state.calls.some(call => call[0] === "sleep" && call[1] === 1000));
  assert.ok(text(byId(view.render(cx), card.id)).includes("Not answered — go to agent"));
  click(byId(view.render(cx), `${card.id}/focus`), cx);
  assert.deepEqual(state.calls.at(-1), ["agent.focus", {terminal:"address"}]);
  assert.deepEqual(state.execs, []);
});

test("Reply input submits literal text via hc send and clears the answered card", async () => {
  const {view, cx, jobs, key, card} = await waitingPanel();
  click(byId(view.render(cx), `${card.id}/reply`), cx);
  const entry = view.blockedReasons.get("address");
  entry.input.emit("focus", cx);
  entry.input.set_value("Use green; keep the label");
  key("2");
  assert.equal(jobs.length, 0);
  state.hcHandler = async args => ({code:0, stdout:args[0] === "get" ? JSON.stringify({status:{state:"working"}}) : "", stderr:""});
  state.hcCalls = [];
  entry.input.emit("submit", cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.hcCalls, [["send", "address", "Use green; keep the label"], ["get", "address"]]);
  assert.equal(view.blockedReasons.has("address"), false);
  assert.equal(view.replyFocused, null);
});

test("waiting card handles no choices, failed sends, changed prompts and stale seq without sending", async () => {
  const {view, cx, jobs, card} = await waitingPanel({screen:"No numbered prompt"});
  assert.equal(byId(view.render(cx), `${card.id}/1`), undefined);
  view.answerAgent(card, "1", cx);
  assert.equal(jobs.length, 0);
  const entry = view.blockedReasons.get("address");
  view.replyTo(card, cx); entry.input.set_value("Try again");
  state.hcHandler = async () => ({code:1, stdout:"", stderr:"Terminal unavailable"});
  view.answerAgent(card, "Try again", cx, true);
  await jobs.shift()(cx);
  assert.ok(text(byId(view.render(cx), card.id)).includes("Terminal unavailable"));
  assert.equal(entry.busy, false);
  state.agents[0].seq++;
  state.hcCalls = [];
  view.answerAgent(card, "Try again", cx, true);
  assert.deepEqual(state.hcCalls, []);
  entry.seq++;
  state.hcHandler = async args => ({code:0, stderr:"", stdout:args[0] === "get"
    ? JSON.stringify({status:{state:"blocked", kind:"question", msg:"answer: New question?"}}) : "1. New answer\n2. Other answer"});
  view.answerAgent(card, "Try again", cx, true);
  await jobs.shift()(cx);
  assert.equal(entry.question, "New question?");
  assert.equal(entry.choices[0].label, "New answer");
  assert.equal(entry.feedback, null);
});

test("blocked fetches stay outside render and refresh can reload the same seq", async () => {
  const {view, cx, jobs, timers} = await waitingPanel();
  state.hcCalls = [];
  view.render(cx); view.render(cx);
  assert.deepEqual(state.hcCalls, []);
  assert.equal(jobs.length, 0);
  timers.find(timer => timer.ms === 1000).fn(cx);
  assert.equal(jobs.length, 0);
  view.syncBlocked(view.liveState().rows, cx, true);
  await jobs.shift()(cx);
  assert.deepEqual(state.hcCalls, [["get", "address"], ["read", "address"]]);
});

test("v3 MR action reasons, ready state and diff render while v2 labels stay unchanged", () => {
  for (const [fields, expected, tone] of [[{ci:"failed"}, "CI failed", "blocked"], [{conflict:true}, "conflict", "blocked"],
    [{unresolved:2}, "2 threads", "blocked"], [{ready:true}, "ready to merge", "accent"], [{draft:true}, "draft", "dim"],
    [{ci:"running"}, "CI running", "working"]]) {
    const mr = {id:"mr", kind:"mr", label:"!123 Demo", state:"opened", ci:"success", conflict:false, unresolved:0, draft:false, ready:false, ...fields};
    const tree = {id:"tree", kind:"tree", task:"Demo", label:"Demo tree", diff:{adds:214, dels:38, files:3}, children:[mr]};
    const task = {id:"task", kind:"task", title:"Demo", children:[tree]};
    const {view, cx} = setup({data:{version:3, rows:[task]}});
    if (tone === "blocked") assert.equal(view.sections().find(section => section.id === "needs").rows[0], task);
    if (fields.ready || tone === "blocked") {
      const summary = byId(view.render(cx), "task-task");
      assert.ok(text(summary).includes("!123"));
      assert.ok(text(summary).includes(expected));
    }
    view.showDetails(task, cx);
    const item = view.detailItems(task).find(item => item.type === "tree");
    const line = byId(view.render(cx), item.id);
    assert.ok(text(line).includes(expected));
    assert.ok(nodes(line).some(node => node.nodes.includes(expected) && node.attributes.text_color?.[0] === tone));
    assert.ok(text(line).includes("+214 −38"));
    assert.ok(nodes(line).some(node => node.nodes.includes("+214") && node.attributes.text_color?.[0] === "done"));
    tree.diff = {adds:0, dels:0};
    assert.ok(!text(byId(view.render(cx), item.id)).includes("+0"));
  }
  const mr = {id:"mr", kind:"mr", label:"!123 Demo", state:"opened", body:"ci✗"};
  const task = {id:"task", kind:"task", children:[mr]};
  const {view, cx} = setup({data:{version:2, rows:[task]}});
  assert.equal(view.sections()[0].id, "progress");
  assert.ok(text(byId(view.render(cx), "task-task")).includes("CI failed"));
});

test("nested agent groups retain 28/24px rows, quiet text and border guides; only blocked marks breathe", () => {
  const agent = {id:"agent", kind:"agent", terminal:"a"};
  const task = {id:"task", kind:"task", children:[{id:"tree", kind:"tree", children:[agent]}, {...agent, id:"main", terminal:"b"}]};
  const {view, cx} = setup({data:{rows:[task]}});
  state.agents = [{address:"a", status:"blocked", seq:1}, {address:"b", status:"idle", seq:1}];
  view.showDetails(task, cx);
  const rendered = view.render(cx), items = view.detailItems(task);
  assert.equal(byId(rendered, items.find(item => item.type === "tree").id).attributes.h[0], 28);
  for (const item of items.filter(item => item.type === "agent")) {
    const line = byId(rendered, item.id);
    assert.equal(line.attributes.h[0], 24);
    assert.equal(line.attributes.text_color[0], "muted");
    assert.ok(nodes(rendered).some(node => node.nodes.includes(line) && node.attributes.border_l_1 && node.attributes.border_color[0] === "border"));
  }
  for (const node of nodes(rendered).filter(node => node.attributes.loop)) {
    assert.equal(node.attributes.bg[0], "blocked");
    assert.deepEqual(node.attributes.loop, ["opacity", {from:0.35, to:1, period:1200}]);
  }
  assert.ok(items.find(item => item.type === "main").groupStart);
});

test("cmd-k opens a centered dialog; task/tree/MR jumps reveal the section, expand the task and focus the selected row", () => {
  const mr = {id:"mr", kind:"mr", label:"!123 Demo MR", state:"opened", url:"https://example.com/123"};
  const tree = {id:"tree", kind:"tree", branch:"feature/button", children:[mr]};
  const task = {id:"task", kind:"task", title:"Demo refactor", children:[tree]};
  const data = {project:"demo", rows:[task], projects:[{name:"demo"}, {name:"other"}]};
  for (const kind of ["task", "tree", "mr"]) {
    const panel = setup({data}); panel.view.project = "demo";
    panel.view.collapsed.add("progress");
    panel.view.key({key:"k", keystroke:"cmd-k", modifiers:{platform:true}}, panel.cx);
    const spec = state.overlays.at(-1);
    assert.equal(spec.kind, "dialog"); assert.equal(spec.props.form, "jump");
    assert.equal(spec.width, 560);
    assert.equal(spec.height, 0.6);
    assert.equal(spec.at, undefined);
    const popup = setup({props:{...spec.props, overlay:"jump"}});
    popup.view.inputs.search.set_value(kind === "task" ? "dm rfct" : kind === "tree" ? "btn" : "123");
    popup.view.inputs.search.emit("change", popup.cx);
    assert.ok(text(popup.view.render(popup.cx)).includes(kind === "task" ? "Demo refactor" : kind === "tree" ? "Worktree" : "!123 Demo MR"));
    const target = popup.view.jumpMatches().findIndex(item => item.kind === kind);
    for (let index = 0; index < target; index++) popup.key("down");
    popup.view.inputs.search.emit("submit", popup.cx);
    state.props = {};
    panel.view.handle.focused = false;
    panel.view.receiveJump(panel.cx);
    assert.equal(panel.view.expanded, "task");
    assert.equal(panel.view.selected, kind === "task" ? "task" : panel.view.detailItems(task).find(item => item.row?.id === "tree").id);
    assert.equal(panel.view.collapsed.has("progress"), false);
    assert.equal(panel.view.handle.is_focused(), true);
    const rendered = panel.view.render(panel.cx);
    assert.ok(byId(rendered, "berbr-tasks").attributes.overflow_y_scrollbar);
    assert.ok(byId(rendered, kind === "task" ? "task-task" : panel.view.selected));
    assert.ok(!state.calls.some(call => call[0] === "scroll"));
    assert.deepEqual(state.execs, []);
  }
});

test("a project chosen in the picker switches the panel outside render", async () => {
  const {view, cx} = setup();
  state.shared.set("berbr.project", "other");
  // As in Herdr.app: spawn runs its body at once, and notifying while rendering throws.
  let rendering = false;
  const started = [];
  const real = {...cx, notify() { if (rendering) throw new Error("notify during render"); },
    spawn: fn => { started.push(fn(real)); }};
  real.sleep = async () => { await new Promise(resolve => setTimeout(resolve, 0)); };
  rendering = true;
  view.render(real);
  rendering = false;
  assert.equal(started.length, 1);
  await started.shift();
  assert.equal(view.project, "other");
  assert.ok(state.execs.some(([, args]) => args.includes("app-snapshot") && args[args.indexOf("--project") + 1] === "other"),
    "the switch loads the new project itself, not on the next 30-second refresh");
});

test("choosing a project mid-load starts it at once and drops the old project's snapshot", async () => {
  const {view, cx, jobs} = setup();
  const pending = [];
  state.execHandler = (_program, args) => new Promise(resolve => pending.push({project:args[args.indexOf("--project") + 1], resolve}));
  view.project = "demo";
  view.refresh(cx);
  const old = jobs.shift()(cx);
  assert.equal(view.loading, true);
  state.shared.set("berbr.project", "other");
  view.render(cx);
  await jobs.shift()(cx);
  const next = jobs.shift()(cx);
  assert.deepEqual(pending.map(run => run.project), ["demo", "other"]);
  const reply = project => ({code:0, stderr:"", stdout:JSON.stringify({project, rows:[], projects:[{name:"demo"}, {name:"other"}]})});
  pending[1].resolve(reply("other"));
  await next;
  assert.equal(view.data.project, "other");
  pending[0].resolve(reply("demo"));
  await old;
  assert.equal(view.project, "other");
  assert.equal(view.data.project, "other");
  assert.equal(state.shared.get("berbr.project"), "other");
});

test("switching back shows the project's last snapshot at once, then the fresh one", async () => {
  const {view, cx, jobs} = setup();
  const pending = [];
  state.execHandler = (_program, args) => new Promise(resolve => pending.push({project:args[args.indexOf("--project") + 1], resolve}));
  const reply = (project, title) => ({code:0, stderr:"",
    stdout:JSON.stringify({project, rows:[{kind:"task", id:`task:${title}`, task:title, title}], projects:[{name:"demo"}, {name:"other"}]})});
  view.project = "demo";
  view.refresh(cx);
  const first = jobs.shift()(cx);
  pending.shift().resolve(reply("demo", "old"));
  await first;
  jobs.length = 0;
  state.shared.set("berbr.project", "other");
  view.switchProject("other", cx);
  assert.equal(view.data, null, "never shown yet: nothing to show");
  const other = jobs.shift()(cx);
  pending.shift().resolve(reply("other", "theirs"));
  await other;
  jobs.length = 0;
  state.shared.set("berbr.project", "demo");
  view.switchProject("demo", cx);
  assert.equal(view.project, "demo");
  assert.equal(view.data.rows[0].title, "old", "the last snapshot, without waiting");
  const fresh = jobs.shift()(cx);
  pending.shift().resolve(reply("demo", "new"));
  await fresh;
  assert.equal(view.data.rows[0].title, "new");
});

test("jump opens loose links, focuses agents, switches projects and closes on Escape", async () => {
  const data = {project:"demo", rows:[{kind:"ticket", id:"ticket", target:"DEMO-12", url:"https://example.com/ticket"},
    {kind:"mr", id:"mr", label:"!123", url:"https://example.com/mr"}, {kind:"agent", id:"agent", terminal:"terminal"}], projects:[{name:"other"}]};
  for (const kind of ["ticket", "mr", "agent", "project"]) {
    const {view, cx, jobs, key} = setup({props:{form:"jump", project:"demo", snapshot:data, overlay:"jump"}});
    const item = view.jumpMatches().find(item => item.kind === kind);
    view.chooseJump(cx, item);
    if (["ticket", "mr"].includes(kind)) {
      await jobs.shift()(cx);
      assert.deepEqual(state.execs, [["open", [item.row.url]]]);
    } else if (kind === "agent") assert.ok(state.calls.some(call => call[0] === "agent.focus" && call[1].terminal === "terminal"));
    else assert.equal(state.shared.get("berbr.project"), "other");
    key("escape");
    assert.deepEqual(state.calls.at(-1), ["closeOverlay", "jump"]);
  }
});

test("an in-flight answer survives a newer seq and keeps same-message feedback after the deferred check", async () => {
  const {view, cx, jobs, card} = await waitingPanel();
  let finishDelay, started;
  const sleeping = new Promise(resolve => {started = resolve;});
  cx.sleep = () => new Promise(resolve => {finishDelay = resolve; started();});
  view.answerAgent(card, "2", cx);
  const pending = jobs.shift()(cx);
  await sleeping;
  state.agents[0].seq++;
  view.syncBlocked(view.liveState().rows, cx);
  assert.equal(jobs.length, 0);
  finishDelay();
  await pending;
  const entry = view.blockedReasons.get("address");
  assert.equal(entry.seq, 2);
  assert.equal(entry.feedback, "Not answered — go to agent");
  view.syncBlocked(view.liveState().rows, cx);
  assert.equal(jobs.length, 0);
});

test("MR jumps reveal history rows and merged labels remain dim despite stale v3 flags", () => {
  const mr = {id:"history", kind:"mr", state:"merged", ready:true, ci:"failed", label:"!123 Demo"};
  const task = {id:"task", kind:"task", children:[mr]};
  const {view, cx} = setup({data:{project:"demo", rows:[task]}});
  view.project = "demo";
  state.shared.set("berbr.jumpTarget", {project:"demo", task:"task", row:"history", kind:"mr"});
  view.receiveJump(cx);
  assert.ok(view.history.has("task"));
  assert.equal(view.chosen().row.id, "history");
  assert.equal(view.mrState(theme(), mr).attributes.text_color[0], "dim");
});

test("jumping to a subtask reveals its row inside the containing task without opening its note", () => {
  const child = {kind:"task", id:"child", title:"Demo child", note:"/home/dev/demo/child.md"};
  const task = {kind:"task", id:"task", title:"Demo parent", children:[child]};
  const data = {project:"demo", rows:[task]};
  const panel = setup({data}); panel.view.project = "demo";
  const popup = setup({props:{form:"jump", project:"demo", snapshot:data, overlay:"jump"}});
  popup.view.chooseJump(popup.cx, popup.view.jumpMatches().find(item => item.row?.id === "child"));
  state.props = {};
  panel.view.receiveJump(panel.cx);
  assert.equal(panel.view.expanded, "task");
  assert.equal(panel.view.chosen().row.id, "child");
  assert.deepEqual(state.overlays, []);
  assert.deepEqual(state.execs, []);
});

test("worktree metadata has a bounded rail and full tooltip while action slots remain fixed", () => {
  const {view} = setup({data:{rows:[]}});
  for (const [state, fields, flag] of [["opened", {ci:"running"}, "CI running"],
    ["opened", {ready:true}, "ready to merge"], ["merged", {}, "merged"]]) {
    const mr = {kind:"mr", label:"!12345 Demo change", state, ...fields};
    const row = {kind:"tree", label:"demo-config-folder-with-a-long-name", task:"Demo", diff:{adds:468, dels:63}};
    const line = byId(view.detailRow(theme(), {id:"tree-priority", type:"tree", row, mr}), "tree-priority");
    const name = line.nodes[1], right = line.nodes[2];
    const link = byId(line, "tree-priority/mr"), iid = link.nodes.find(node => text(node) === "!12345");
    const flagText = link.nodes.find(node => nodes(node).some(child => child.attributes.accessibility_label?.[0]?.startsWith(flag)));
    const diff = right.nodes.find(node => text(node) === "+468 −63");
    assert.ok(iid.attributes.flex_none);
    assert.equal(iid.attributes.truncate, undefined);
    assert.ok(diff.attributes.flex_none);
    assert.ok(diff.nodes.every(node => node.attributes.flex_none && !node.attributes.truncate));
    assert.ok(flagText.attributes.flex_none);
    assert.equal(flagText.attributes.truncate, undefined);
    assert.equal(flagText.attributes.w, undefined);
    assert.equal(right.attributes.min_w[0], 0);
    assert.equal(right.attributes.max_w[0], "35%");
    assert.ok(right.attributes.overflow_hidden);
    assert.ok(link.attributes.overflow_hidden);
    assert.ok(right.attributes.tooltip[0].includes(flag));
    assert.ok(line.nodes.at(-1).attributes.flex_none);
    assert.ok(name.attributes.truncate);
    assert.equal(name.attributes.min_w[0], 0);
    assert.ok(name.attributes.flex_1);
  }
  const line = byId(view.detailRow(theme(), {id:"tree-no-mr", type:"tree", row:{kind:"tree", label:"Demo tree"}}), "tree-no-mr");
  const noMr = line.nodes[2].nodes.find(node => text(node) === "no MR");
  assert.ok(noMr.attributes.flex_none);
  assert.equal(noMr.attributes.truncate, undefined);
});

test("Open MRs and History keep iid and state fixed while the separate title truncates", () => {
  const {view} = setup({data:{rows:[]}});
  for (const [historical, state, fields, flag] of [[false, "opened", {ci:"running"}, "CI running"],
    [false, "opened", {ready:true}, "ready to merge"], [true, "merged", {}, "merged"], [true, "closed", {}, "closed"]]) {
    const row = {kind:"mr", label:"!12345 Demo change with a long title", state, ...fields};
    const rendered = view.detailRow(theme(), {id:"mr-fixed", type:"mr", row, historical, tickets:[]});
    const link = byId(rendered, "mr-fixed/mr");
    const iid = link.nodes.find(node => text(node) === "!12345");
    const title = link.nodes.find(node => text(node) === "Demo change with a long title");
    const status = link.nodes.find(node => nodes(node).some(child => child.attributes.accessibility_label?.[0]?.startsWith(flag)));
    for (const node of [iid, status]) {
      assert.ok(node.attributes.flex_none);
      assert.equal(node.attributes.truncate, undefined);
    }
    assert.ok(title.attributes.flex_1);
    assert.equal(title.attributes.min_w[0], 0);
    assert.ok(title.attributes.truncate);
  }
});

test("agent name and status truncate within their budgets with full tooltips", () => {
  const {view} = setup({data:{rows:[]}});
  for (const [status, label] of [["blocked", "Waiting for you"], ["done", "finished"], [null, "stopped"]]) {
    const live = status ? {address:"demo", status, title:"Demo agent with a long name"} : null;
    const rendered = view.detailRow(theme(), {id:"agent-fixed", type:"agent", row:{label:"Demo stopped agent"}, live, status});
    const line = byId(rendered, "agent-fixed"), name = line.nodes[1], right = line.nodes[2];
    const statusText = right.nodes.find(node => text(node) === label);
    assert.equal(right.attributes.max_w[0], "50%");
    assert.equal(right.attributes.min_w[0], 0);
    assert.ok(statusText.attributes.truncate);
    assert.equal(statusText.attributes.tooltip[0], label);
    assert.ok(name.attributes.flex_1);
    assert.equal(name.attributes.min_w[0], 0);
    assert.ok(name.attributes.truncate);
  }
});

test("uncommitted files use a 6px warning dot before the line diff, with singular and plural tooltips", () => {
  const {view} = setup({data:{rows:[]}});
  for (const count of [0, 1, 2]) for (const diff of [null, {adds:376, dels:38, files:4}]) {
    const mr = {kind:"mr", label:"!12345 Demo change", state:"merged"};
    const row = {kind:"tree", tree:"demo", body:`/home/dev/demo\n +${count} ↑3`, diff};
    const rendered = view.detailRow(theme(), {id:"dirty-tree", type:"tree", row, mr});
    const right = byId(rendered, "dirty-tree").nodes[2];
    const tooltip = `${count} uncommitted ${count === 1 ? "file" : "files"}`;
    const dot = right.nodes.find(node => node.attributes.tooltip?.[0] === tooltip);
    assert.equal(Boolean(dot), count > 0);
    assert.ok(!nodes(right).some(node => node.nodes.includes(`+${count}`)));
    assert.ok(text(right).includes("↑3"));
    if (!count) continue;
    assert.equal(dot.attributes.size[0], 6);
    assert.equal(dot.attributes.bg[0], "working");
    assert.equal(dot.attributes.accessibility_label[0], tooltip);
    assert.ok(dot.attributes.flex_none);
    if (diff) assert.equal(text(right.nodes[right.nodes.indexOf(dot) + 1]), "+376 −38");
  }
});

test("loose and Cleanup worktree marks spell out uncommitted counts without a plus sign", () => {
  const {view} = setup({data:{rows:[]}});
  for (const count of [1, 2]) {
    const row = {kind:"tree", tree:"demo", body:`/home/dev/demo\n +${count} ↑3`};
    const summary = view.looseContent(theme(), row, "worktree left");
    const detail = view.detailRow(theme(), {id:"loose-dirty", type:"tree", row, loose:true});
    for (const rendered of [summary, detail]) {
      assert.ok(text(rendered).includes(`${count} uncommitted`));
      assert.ok(!text(rendered).includes(`+${count}`));
      assert.ok(text(rendered).includes("↑3"));
      const mark = nodes(rendered).find(node => node.nodes.includes(`${count} uncommitted`));
      assert.equal(mark.attributes.text_color[0], "working");
      assert.ok(!nodes(rendered).some(node => node.attributes.size?.[0] === 6));
    }
  }
});

test("spaces named by title or old id suppress Start work and focus the matching space", () => {
  for (const name of [" demo-4 ", " проверка PARSER "]) {
    const task = {kind:"task", id:"demo", task:"demo-4", title:"Проверка Parser", children:[]};
    const {view, cx, key} = setup({data:{rows:[task]}, liveSpaces:[{id:"existing-space", name, tabs:[]}]});
    assert.ok(!view.detailItems(task).some(item => item.label === "Start work"));
    key("enter");
    assert.deepEqual(state.calls, [["space.select", {id:"existing-space"}]]);
    const control = byId(view.render(cx), "task-space-demo");
    assert.ok(control);
    state.calls.length = 0;
    click(control, cx);
    assert.deepEqual(state.calls, [["space.select", {id:"existing-space"}]]);
  }
});

test("right click selects task, detail and loose rows without expanding or running actions", () => {
  const tree = {id:"tree", kind:"tree", tree:"demo-tree", task:"demo", cwd:"/home/dev/demo"};
  const agent = {id:"agent", kind:"agent", terminal:"pane"};
  const mr = {id:"mr", kind:"mr", state:"opened", url:"https://gitlab.example/demo/-/merge_requests/1"};
  const ticket = {id:"ticket", kind:"ticket", target:"DEMO-1", url:"https://jira.example/browse/DEMO-1"};
  const task = {id:"task", kind:"task", task:"demo", title:"Demo", children:[tree, agent, mr, ticket]};
  const {view, cx, jobs} = setup({data:{rows:[task]}});
  const right = node => {
    assert.equal(node.attributes.on_mouse_down[0], "right");
    node.attributes.on_mouse_down[1]({}, cx);
  };
  right(view.taskRow(theme(), task));
  assert.equal(view.selected, "task");
  assert.equal(view.expanded, null);
  assert.equal(state.overlays.at(-1).kind, "popup");
  assert.equal(state.overlays.at(-1).at, "pointer");
  assert.equal(state.overlays.at(-1).width, 220);
  assert.equal(state.overlays.at(-1).height, undefined);
  assert.equal(state.overlays.at(-1).props.form, "menu");
  view.showDetails(task, cx);
  for (const row of [tree, agent, mr]) {
    const item = view.detailItems(task).find(item => item.row === row);
    right(byId(view.detailRow(theme(), item), item.id));
    assert.equal(view.selected, item.id);
    assert.equal(state.overlays.at(-1).props.row, row);
  }
  for (const row of [tree, agent, mr, ticket]) {
    right(view.looseRow(theme(), row, "cleanup"));
    assert.equal(state.overlays.at(-1).props.row, row);
  }
  // Ticket attributes are inline links, rather than navigable detail wrappers.
  right(view.externalLink(theme(), "inline-ticket", ticket));
  view.render(cx);
  assert.equal(view.selected, ticket.id);
  assert.equal(view.chosen(), ticket);
  assert.deepEqual(state.calls, []);
  assert.deepEqual(state.execs, []);
  assert.deepEqual(state.hcCalls, []);
  assert.equal(jobs.length, 0);
});

test("headers and plain detail rows never open a menu", () => {
  const {view, cx} = setup();
  for (const item of [{kind:"header", id:"header"}, {kind:"section", id:"section"},
    {kind:"detail", type:"main", id:"main"}, {kind:"detail", type:"action", id:"action"}]) {
    view.openRowMenu(item, cx);
    if (item.kind === "detail") assert.equal(view.innerRow(theme(), item).attributes.on_mouse_down, undefined);
  }
  assert.deepEqual(state.overlays, []);
});

test("Shift F10 and context-menu keys open the selected row menu outside text inputs", () => {
  const task = {id:"task", kind:"task", task:"demo", title:"Demo"};
  const {view, cx} = setup({data:{rows:[task]}});
  for (const event of [{key:"f10", keystroke:"shift-f10"}, {key:"contextmenu"}, {key:"menu"}, {key:"apps"}]) {
    view.key(event, cx);
    assert.equal(state.overlays.at(-1).props.row, task);
  }
  assert.equal(state.overlays.length, 4);
  view.searchFocused = true;
  view.key({key:"f10", keystroke:"shift-f10"}, cx);
  assert.equal(state.overlays.length, 4);
});

test("menu arrows wrap, Enter dispatches the highlighted item, and Escape only closes", () => {
  const row = {kind:"agent", terminal:"pane"};
  const {view, cx, key, jobs} = setup({props:{form:"menu", row, overlay:"menu"}});
  key("up");
  assert.equal(view.menuIndex, 2);
  assert.ok(byId(view.render(cx), "menu-stop-agent").attributes.bg);
  key("down"); assert.equal(view.menuIndex, 0);
  key("down"); key("down"); key("enter");
  assert.equal(state.overlays.at(-1).props.form, "stop-agent");
  assert.deepEqual(state.hcCalls, []);
  assert.equal(jobs.length, 0);
  key("escape");
  assert.deepEqual(state.calls.at(-1), ["closeOverlay", "menu"]);
});

test("new agent menu dispatch adds an agent in the existing worktree and closes the overlay", async () => {
  const row = {kind:"tree", tree:"demo-tree", task:"demo"};
  const {view, cx, jobs} = setup({props:{form:"menu", row, overlay:"menu"}});
  const calls = [];
  view.cli = async args => {calls.push(args); return "Started";};
  view.chooseMenu(cx, "new-agent");
  assert.equal(jobs.length, 1);
  await jobs.shift()(cx);
  assert.deepEqual(calls, [["add", "demo", "--in", "demo-tree"]]);
  assert.ok(state.calls.some(call => call[0] === "closeOverlay" && call[1] === "menu"));
});

test("active worktree menu dispatch opens the existing remove dialog without running done", () => {
  const mr = {kind:"mr", label:"!1 Demo", state:"opened"};
  const row = {id:"tree", kind:"tree", tree:"demo-tree", task:"demo", children:[mr]};
  const rows = [{kind:"task", task:"demo"}];
  const {view, cx, jobs} = setup({props:{form:"menu", row, mr, rows, overlay:"menu"}});
  click(byId(view.render(cx), "menu-remove-tree"), cx);
  assert.equal(state.overlays.at(-1).props.form, "remove-tree");
  assert.equal(state.overlays.at(-1).props.tree, row);
  assert.equal(state.overlays.at(-1).props.rows, rows);
  assert.deepEqual(state.calls, [["closeOverlay", "menu"]]);
  assert.equal(jobs.length, 0);
});

test("stop agent opens a confirmation and only its explicit confirm button calls hc kill", async () => {
  const row = {kind:"agent", terminal:"pane"};
  const menu = setup({props:{form:"menu", row, live:{address:"address"}, overlay:"menu"}});
  menu.view.chooseMenu(menu.cx, "stop-agent");
  const spec = state.overlays.at(-1);
  assert.equal(spec.modal, true);
  assert.equal(spec.props.terminal, "address");
  assert.deepEqual(state.hcCalls, []);
  const cancel = setup({props:{...spec.props, overlay:"confirm"}});
  cancel.key("enter"); cancel.key("escape");
  assert.deepEqual(state.hcCalls, []);
  assert.equal(cancel.jobs.length, 0);
  const confirm = setup({props:{...spec.props, overlay:"confirm"}});
  click(byId(confirm.view.render(confirm.cx), "stop-agent"), confirm.cx);
  assert.deepEqual(state.hcCalls, []);
  await confirm.jobs.shift()(confirm.cx);
  assert.deepEqual(state.hcCalls, [["kill", "address"]]);
  assert.ok(state.calls.some(call => call[0] === "closeOverlay" && call[1] === "confirm"));
});

test("stop failures retain the dialog and show the hc error without retrying", async () => {
  const {view, cx, jobs} = setup({props:{form:"stop-agent", terminal:"demo", overlay:"confirm"}});
  state.hcResult = {code:1, stdout:"", stderr:"Terminal demo is unavailable"};
  view.submitStopAgent(cx); view.submitStopAgent(cx);
  await jobs.shift()(cx);
  assert.deepEqual(state.hcCalls, [["kill", "demo"]]);
  assert.ok(text(view.render(cx)).includes("Terminal demo is unavailable"));
  assert.ok(!state.calls.some(call => call[0] === "closeOverlay"));
});

test("copy actions pass literal text through argv to pbcopy and show Copied on success", async () => {
  const value = '-demo "quote" $(literal) `literal`\nnext';
  for (const [row, id] of [
    [{kind:"task", title:value}, "copy-title"],
    [{kind:"tree", tree:"demo", cwd:value}, "copy-path"],
    [{kind:"tree", tree:"demo", body:"/home/dev/demo\n +1"}, "copy-path"],
    [{kind:"tree", tree:"demo", branch:value}, "copy-branch"],
    [{kind:"agent", terminal:value}, "copy-terminal"],
    [{kind:"mr", url:value}, "copy-link"],
  ]) {
    const {view, cx, jobs} = setup({props:{form:"menu", row, overlay:"menu"}});
    view.chooseMenu(cx, id);
    await jobs.shift()(cx);
    assert.deepEqual(state.execs, [["/bin/sh", ["-c", 'printf %s "$1" | /usr/bin/pbcopy', "berbr-copy",
      row.body ? "/home/dev/demo" : value]]]);
    assert.equal(state.notices.at(-1).body, "Copied");
    assert.deepEqual(state.calls.at(-1), ["closeOverlay", "menu"]);
  }
});

test("copy and URL failures remain visible in the menu", async () => {
  for (const id of ["copy-link", "url"]) {
    const {view, cx, jobs} = setup({props:{form:"menu", row:{kind:"mr", url:"https://example.test/demo"}, overlay:"menu"}});
    state.execResult = {code:1, stdout:"", stderr:"Example failure"};
    view.chooseMenu(cx, id);
    await jobs.shift()(cx);
    assert.ok(text(view.render(cx)).includes("Example failure"));
    assert.ok(!state.calls.some(call => call[0] === "closeOverlay"));
    assert.equal(state.notices.length, 0);
  }
});

test("rename menu opens a prefilled form and submits only berbr rename with literal argv", async () => {
  const row = {kind:"task", task:"demo", title:"Demo title"};
  const menu = setup({props:{form:"menu", row, overlay:"menu"}});
  menu.view.chooseMenu(menu.cx, "rename");
  const spec = state.overlays.at(-1);
  assert.equal(spec.props.form, "rename");
  assert.equal(spec.props.prefill.title, "Demo title");
  const {view, cx, jobs} = setup({props:{...spec.props, overlay:"rename"}});
  const calls = [];
  view.cli = async args => {calls.push(args); return "Renamed";};
  view.values.title = "  ";
  click(byId(view.render(cx), "rename-task"), cx);
  assert.equal(jobs.length, 0);
  view.inputs.title.set_value('Demo "new" $(literal)');
  view.inputs.title.emit("change", cx);
  view.inputs.title.emit("submit", cx);
  await jobs.shift()(cx);
  assert.deepEqual(calls, [["rename", "demo", 'Demo "new" $(literal)']]);
});

test("task and tree menu actions reuse space, note, done, select and review flows", async () => {
  const task = {kind:"task", task:"demo", title:"Demo", note:"/home/dev/demo.md"};
  for (const id of ["space", "note", "done"]) {
    const {view, cx} = setup({props:{form:"menu", row:task, overlay:"menu"}, liveSpaces:[{id:"space", name:"Demo"}]});
    view.chooseMenu(cx, id);
    if (id === "space") assert.deepEqual(state.calls.at(-1), ["space.select", {id:"space"}]);
    else assert.equal(state.overlays.at(-1).props.form, id);
  }
  for (const [row, id, expected] of [
    [task, "start", ["select", "demo"]],
    [{kind:"tree", task:"demo", tree:"demo-tree", merged:true}, "resume", ["select", "demo", "--tree", "demo-tree"]],
    [{kind:"tree", task:"demo", tree:"demo-tree"}, "review", ["review", "demo-tree"]],
  ]) {
    const {view, cx, jobs} = setup({props:{form:"menu", row, overlay:"menu"}});
    const calls = [];
    view.cli = async args => {calls.push(args); return "Done";};
    view.chooseMenu(cx, id);
    await jobs.shift()(cx);
    assert.deepEqual(calls, [expected]);
  }
});

test("tree confirmation states known blockers and that pushed unmerged commits may be removed", () => {
  const {view, cx} = setup({props:{form:"remove-tree", tree:{kind:"tree", tree:"demo", body:"/home/dev/demo\n +2"},
    mr:{kind:"mr", state:"opened"}}});
  const rendered = text(view.render(cx));
  assert.ok(rendered.includes("Removal will be refused: 2 uncommitted change(s)"));
  assert.ok(rendered.includes("Removal will be refused: the MR is open"));
  assert.ok(rendered.includes("Remove never discards uncommitted files"));
  assert.ok(rendered.includes("2 uncommitted change(s) are lost for good"));
  assert.ok(rendered.includes("Unmerged commits already pushed to origin can be removed locally"));
});

// Regression scenarios adapted from the synthetic visual/logic audits.
test("V06: expanding a parent preserves descendant questions in both render and keyboard rows", () => {
  const agent = {id:"child-agent", kind:"agent", terminal:"a"};
  const child = {id:"child", kind:"task", task:"Child", children:[agent]};
  const parent = {id:"parent", kind:"task", task:"Parent", children:[child]};
  const {view, cx} = setup({data:{rows:[parent]}});
  state.agents = [{address:"a", status:"blocked", seq:1, title:"Child worker"}];
  view.blockedReasons.set("a", {reason:{kind:"question", detail:"Continue?"}, question:"Continue?", choices:[{key:"1", label:"Yes"}]});
  for (const expanded of [null, parent.id]) {
    view.setExpanded(expanded, cx);
    assert.ok(text(view.render(cx)).includes("Continue?"));
    assert.equal(view.rows().filter(row => row.kind === "waiting").length, 1);
  }
  view.subtasks.add(parent.id);
  assert.ok(text(view.render(cx)).includes("Continue?"));
  assert.equal(view.rows().filter(row => row.kind === "waiting").length, 1);
});

test("V07: unblocking during reply verification clears focus immediately and in finally", async () => {
  const task = {id:"task", kind:"task", task:"Demo", children:[{id:"agent", kind:"agent", terminal:"a"}]};
  const {view, cx, jobs, key} = setup({data:{rows:[task]}});
  state.agents = [{address:"a", status:"blocked", seq:1}];
  const entry = {seq:1, question:"Continue?", msg:"answer: Continue?", choices:[], reason:{kind:"question"}};
  view.blockedReasons.set("a", entry);
  const item = view.waitingItems(task)[0];
  view.replyTo(item, cx); entry.input.set_value("Continue"); entry.input.emit("focus", cx);
  let resume, started;
  const sleeping = new Promise(resolve => {started = resolve;});
  cx.sleep = () => new Promise(resolve => {resume = resolve; started();});
  state.hcHandler = async args => ({code:0, stdout:args[0] === "get" ? JSON.stringify({status:{state:"working"}}) : ""});
  view.answerAgent(item, "Continue", cx, true);
  const job = jobs.shift()(cx); await sleeping;
  state.agents = [{address:"a", status:"working", seq:2}];
  view.syncBlocked(view.rows(), cx);
  assert.equal(view.blockedReasons.has("a"), false);
  assert.equal(view.replyFocused, null);
  resume(); await job;
  assert.equal(entry.busy, false);
  assert.ok(!nodes(view.render(cx)).some(node => node.input === entry.input));
  key("n");
  assert.equal(state.overlays.at(-1).props.form, "new");
});

test("logic 22: the Jira tab requires a picked ticket or a valid key before creating anything", () => {
  for (const ticket of ["", "  ", "DEMO", "demo-1", "DEMO-1 extra"]) {
    const {view, cx, jobs} = setup({props:{form:"new", project:"demo"}});
    view.source = "jira"; view.values.title = ticket;
    view.submitNew(cx);
    assert.match(view.error, /valid Jira ticket key/);
    assert.equal(jobs.length, 0);
    assert.deepEqual(state.execs, []);
  }
});

test("V10: linked Jira offers a name or existing worktree and passes the explicit backend flag", async () => {
  for (const existing of [false, true]) {
    const tree = {kind:"tree", id:"tree", tree:"demo-checkout", cwd:"/home/dev/demo"};
    const {view, cx, jobs} = setup({props:{form:"ticket", task:"Demo", tickets:["DEMO-1"], trees:[tree], prefill:{ticket:"DEMO-1"}}});
    assert.ok(nodes(view.render(cx)).some(node => node.input === view.inputs.name));
    view.submitTicket(cx, true);
    assert.match(view.error, /worktree name/); assert.equal(jobs.length, 0);
    if (existing) click(byId(view.render(cx), "ticket-tree-tree"), cx);
    else view.values.name = "second-checkout";
    const calls = []; view.cli = async args => {calls.push(args); return "Added";};
    view.submitTicket(cx, true); await jobs.shift()(cx);
    assert.deepEqual(calls, [["add", "Demo", "DEMO-1", existing ? "--in" : "--name", existing ? tree.tree : "second-checkout"]]);
  }
});

test("V01/V02/V03: long branches and questions have budgets and tooltips, and action strips wrap", () => {
  const {view} = setup();
  const branch = "feature/" + "Ж".repeat(150);
  const row = {kind:"tree", id:"tree", tree:"checkout", branch, body:"/home/dev/demo\n +12 ↑345 quiet 999d"};
  const line = byId(view.detailRow(theme(), {id:"loose-tree", type:"tree", row, loose:true}), "loose-tree");
  const rail = line.nodes[2], label = nodes(rail).find(node => node.nodes.includes(branch));
  assert.equal(rail.attributes.max_w[0], "35%");
  assert.ok(rail.attributes.overflow_hidden);
  assert.ok(label.attributes.truncate); assert.equal(label.attributes.tooltip[0], branch);
  assert.ok(line.nodes.at(-1).attributes.flex_none);
  const detail = "Проверьте разрешение на изменение длинного файла ".repeat(6);
  view.blockedReasons.set("a", {reason:{kind:"question", detail}});
  const agent = byId(view.detailRow(theme(), {id:"agent", type:"agent", row:{label:"Исполнитель"},
    live:{address:"a", status:"blocked", title:"Работа"}, status:"blocked"}), "agent");
  assert.equal(agent.nodes[2].attributes.max_w[0], "50%");
  assert.equal(agent.nodes[2].nodes.at(-1).attributes.tooltip[0], `Asks: ${detail}`);
  assert.ok(agent.nodes[2].nodes.at(-1).attributes.truncate);
  const actions = view.actionRows(theme(), view.looseItems({id:"ticket", kind:"ticket", target:"DEMO-1", state:"New"}));
  assert.ok(actions.attributes.flex_wrap);
  assert.equal(actions.attributes.h, undefined);
});

test("V04: confirm and segmented controls are tab stops with Enter/Space activation; Escape cancels", async () => {
  for (const key of ["enter", "space", " "]) {
    const {view, cx, jobs} = setup({props:{form:"stop-agent", terminal:"demo", overlay:"stop"}});
    const confirm = byId(view.render(cx), "stop-agent");
    assert.equal(confirm.attributes.tab_index[0], 0);
    assert.equal(confirm.attributes.tab_stop[0], true);
    confirm.attributes.on_key_down[0]({key}, cx);
    assert.equal(jobs.length, 1);
    const disabled = byId(view.render(cx), "stop-agent");
    assert.equal(disabled.attributes.tab_stop[0], false);
    disabled.attributes.on_key_down[0]({key}, cx);
    assert.equal(jobs.length, 1);
    await jobs.shift()(cx);
  }
  for (const [form, id] of [["done", "done"], ["remove-tree", "remove-tree"], ["remove-merged", "confirm-remove-merged"]]) {
    const {view, cx, jobs} = setup({props:{form, task:"Demo", row:{kind:"task", task:"Demo", children:[]},
      tree:{kind:"tree", tree:"demo"}, trees:[]}});
    view.busy = true;
    const confirm = byId(view.render(cx), id);
    assert.equal(confirm.attributes.tab_index[0], 0);
    assert.equal(confirm.attributes.tab_stop[0], false);
    confirm.attributes.on_key_down[0]({key:"enter"}, cx);
    assert.equal(jobs.length, 0);
  }
  const {view, cx, key} = setup({props:{form:"new", overlay:"new"}});
  let root = view.render(cx);
  for (const [id, read, expected] of [["source-jira", () => view.source, "jira"],
    ["agent-choice", () => view.choosing, "agent"], ["agent-claude", () => view.agent, "claude"]]) {
    const control = byId(root, id);
    assert.equal(control.attributes.tab_index[0], 0);
    control.attributes.on_key_down[0]({key:"space"}, cx);
    assert.equal(read(), expected);
    root = view.render(cx);
  }
  key("escape"); assert.deepEqual(state.calls.at(-1), ["closeOverlay", "new"]);
});

test("V08/V09: Jump, Projects and Attach have bounded scroll bodies and keyboard selection", async () => {
  const rows = Array.from({length:50}, (_, index) => ({kind:"task", id:`task-${index}`, name:`Demo ${index}`, task:`Task ${index}`, title:`Задача ${index}`}));
  for (const [form, listId] of [["jump", "jump-results"], ["projects", "project-results"], ["attach-task", "attach-results"]]) {
    const {view, cx, jobs, key} = setup({props:{form, projects:rows, tasks:rows, snapshot:{rows}, ticket:"DEMO-1", overlay:"picker"}});
    let root = view.render(cx), list = byId(root, listId);
    // Jump is a dialog with a height; the pickers are content-sized popups
    if (form === "jump") { assert.ok(list.attributes.flex_1); assert.equal(list.attributes.min_h[0], 0); }
    else assert.ok(list.attributes.h[0] > 0 && !list.attributes.flex_1);
    assert.ok(list.attributes.overflow_y_scrollbar);
    for (let index = 0; index < 40; index++) key("down");
    root = view.render(cx); list = byId(root, listId);
    assert.ok(list.nodes[0].attributes.bg, "keyboard selection is the first rendered result");
    if (form === "projects") {
      key("enter"); assert.equal(state.shared.get("berbr.project"), rows[40].name);
    } else if (form === "attach-task") {
      const calls = []; view.cli = async args => {calls.push(args); return "Attached";};
      key("enter"); await jobs.shift()(cx);
      assert.deepEqual(calls, [["attach", "DEMO-1", rows[40].task]]);
    } else {
      const chosen = view.jumpMatches()[view.jumpIndex];
      key("enter"); assert.equal(state.shared.get("berbr.jumpTarget").row, chosen.row.id);
    }
  }
});

test("V11: text buttons and MR links keep an equal spinner slot while idle or busy", () => {
  const idleButton = button(theme(), "create", "Create task", () => {}, "primary");
  const busyButton = button(theme(), "create", "Create task", () => {}, "primary", true, true);
  assert.equal(idleButton.nodes.length, busyButton.nodes.length);
  assert.equal(idleButton.nodes.at(-1).attributes.size[0], 11);
  assert.equal(busyButton.nodes.at(-1).attributes.size[0], 11);
  const {view} = setup();
  const mr = {kind:"mr", id:"mr", label:"!1", state:"opened"};
  const idle = view.externalLink(theme(), "link", mr).child("!1");
  view.busy = true; view.busyAction = "link";
  const busy = view.externalLink(theme(), "link", mr).child("!1");
  assert.equal(idle.nodes.length, busy.nodes.length);
  assert.equal(idle.nodes[0].attributes.size[0], 11);
  assert.equal(busy.nodes[0].attributes.size[0], 11);
  assert.ok(idle.nodes[0].attributes.invisible);
  assert.equal(busy.nodes[0].attributes.invisible, undefined);
});

test("M2: blocked prompt workers and queue are bounded, prioritize expanded work and reuse unchanged seq", async () => {
  const rows = Array.from({length:300}, (_, index) => ({kind:"task", id:`task-${index}`, title:`Demo ${index}`,
    children:[{kind:"agent", terminal:`agent-${index}`}]}));
  const {view, cx, jobs} = setup({data:{rows}});
  state.agents = rows.map((_, index) => ({address:`agent-${index}`, status:"blocked", seq:1}));
  view.expanded = "task-299"; view.selected = null;
  let active = 0, peak = 0;
  state.hcHandler = async args => {
    active++; peak = Math.max(peak, active);
    assert.ok(view.promptQueue.length <= 64);
    await Promise.resolve(); active--;
    return {code:0, stdout:args[0] === "get" ? JSON.stringify({status:{state:"blocked", msg:"Continue?"}}) : "1. Yes\n2. No", stderr:""};
  };
  view.syncBlocked(view.liveState().rows, cx);
  assert.equal(jobs.length, 4); assert.ok(view.promptQueue.length <= 64);
  await Promise.all(jobs.splice(0).map(job => job(cx)));
  assert.equal(peak, 8); // Four prompt workers, two parallel hc requests each.
  assert.equal(state.hcCalls.length, 600);
  assert.equal(state.hcCalls[0][1], "agent-299");
  view.didOnline = true; view.cli = async () => JSON.stringify(view.data);
  view.refresh(cx, true);
  await Promise.all(jobs.splice(0).map(job => job(cx)));
  assert.equal(state.hcCalls.length, 600);
  const entry = view.blockedReasons.get("agent-299"); entry.readAt -= 300001;
  view.syncBlocked(view.liveState().rows, cx);
  await Promise.all(jobs.splice(0).map(job => job(cx)));
  assert.equal(state.hcCalls.length, 602);
  view.refresh(cx, true, true);
  await Promise.all(jobs.splice(0).map(job => job(cx)));
  assert.equal(state.hcCalls.length, 1202);
});

test("M4: note wikilinks use existence checks, unchanged folder refresh skips them and manual reload retries", async () => {
  const path = "/home/dev/notes/main.md";
  const {view, cx, jobs} = setup({props:{form:"note", path}});
  state.files = new Map([[path, "[[page]] [[missing]] [[../outside]]"], ["/home/dev/notes/page.md", "x".repeat(256 * 1024)]]);
  await jobs.shift()(cx);
  assert.deepEqual(state.reads, [path]);
  assert.deepEqual(state.existsCalls, ["/home/dev/notes/page.md", "/home/dev/notes/missing.md"]);
  assert.ok(view.note.existing.has("/home/dev/notes/page.md"));
  const watch = jobs.shift()(cx); await Promise.resolve();
  state.watchers.shift().resolve(["/home/dev/notes"]); await Promise.resolve(); await Promise.resolve();
  await jobs.shift()(cx);
  assert.equal(state.existsCalls.length, 2);
  state.files.set("/home/dev/notes/missing.md", "Found");
  click(byId(view.render(cx), "note-reload"), cx); await jobs.shift()(cx);
  assert.ok(view.note.existing.has("/home/dev/notes/missing.md"));
  state.watchers.shift().reject(new Error("Stopped")); await watch;
});

test("an MR link without a ticket starts with its number: the spinner slot trails the link", () => {
  const task = {kind:"task", id:"task", task:"Demo", children:[
    {kind:"mr", id:"mr", state:"opened", label:"!123 Demo", ci:"running", url:"https://example.invalid/mr/123"}]};
  const {view} = setup({data:{rows:[task]}});
  const link = byId(view.taskRow(theme(), task), "task-mr-task");
  assert.equal(text(link.nodes[0]).trim(), "!123");
  const slot = link.nodes.at(-1);
  assert.deepEqual(slot.attributes.size, [11]);
  assert.ok(slot.attributes.invisible);
});

test("a command from a removed worktree's terminal runs from the project instead", async () => {
  const {view} = setup({props:{opening:{cwd:"/home/dev/demo/.herdr/worktrees/gone"}}});
  view.project = "demo";
  view.data = {projects:[{name:"demo", cwd:"/home/dev/demo"}], rows:[]};
  state.execHandler = async (_program, _args, options) => {
    if (options.cwd?.endsWith("/gone")) throw new Error("herdr.exec: cannot run zsh: No such file or directory (os error 2)");
    return {code:0, stdout:"ok", stderr:""};
  };
  assert.equal(await view.cli(["app-snapshot"]), "ok");
  assert.deepEqual(state.execs.map(call => call[2].cwd), ["/home/dev/demo/.herdr/worktrees/gone", "/home/dev/demo"]);
  state.execHandler = async () => { throw new Error("herdr.exec: permission denied (os error 13)"); };
  await assert.rejects(view.cli(["app-snapshot"]), /os error 13/);
});

test("the mod log keeps changes and failures, and snapshots only in debug", async () => {
  const {logRun, setDebug, logPath, log} = await import("../log.js");
  state.fsFiles = new Map();
  setDebug(false);
  await logRun(["app-snapshot", "--cached"], {cwd:"/home/dev/demo", ms:5, code:0});
  assert.equal(state.fsFiles.get(logPath()), undefined);
  await logRun(["done", "--tree", "demo", "--yes"], {cwd:"/home/dev/demo/gone", ms:7, code:1});
  await logRun(["app-snapshot"], {cwd:"/home/dev/demo/gone", ms:3, code:0, retried:{to:"/home/dev/demo"}});
  await logRun(["select", "Demo"], {ms:2, error:"cannot run zsh: No such file or directory (os error 2)"});
  let text = state.fsFiles.get(logPath());
  assert.match(text, /berbr \["done","--tree","demo","--yes"\] in \/home\/dev\/demo\/gone exited 1 7ms/);
  assert.match(text, /its folder is gone; ran from \/home\/dev\/demo\) ok 3ms/);
  assert.match(text, /could not run: cannot run zsh/);
  setDebug(true);
  await logRun(["app-snapshot", "--cached"], {ms:5, code:0});
  assert.match(state.fsFiles.get(logPath()), /app-snapshot","--cached"\] ok 5ms/);
  setDebug(false);
  await log("x".repeat(600 * 1024));
  await log("newest");
  text = state.fsFiles.get(logPath());
  assert.ok(text.length <= 512 * 1024 && text.endsWith("newest\n"));
});

test("a failed berbr run from the panel lands in the mod log with its stderr", async () => {
  const {logPath} = await import("../log.js");
  state.fsFiles = new Map();
  const {view} = setup({props:{opening:{cwd:"/home/dev/demo"}}});
  state.execHandler = async () => ({code:1, stdout:"", stderr:"Error: \"demo\" is not finished"});
  await assert.rejects(view.cli(["done", "--tree", "demo", "--yes"]), /not finished/);
  await new Promise(resolve => setTimeout(resolve, 0));
  const text = state.fsFiles.get(logPath());
  assert.match(text, /exited 1/);
  assert.match(text, /berbr done: Error: "demo" is not finished/);
});

test("a note from berbr's own folder opens in the default editor, not Obsidian", async () => {
  state.readResult = "Body";
  const {view, cx, jobs} = setup({props:{form:"note", path:"/home/dev/.berbr/projects/demo/tasks/demo-1.md", title:"Demo", vault:false}});
  await jobs.shift()(cx);
  const rendered = view.render(cx);
  assert.equal(byId(rendered, "note-obsidian"), undefined);
  click(byId(rendered, "note-open"), cx);
  await jobs.pop()(cx);
  assert.deepEqual(state.execs, [["open", ["/home/dev/.berbr/projects/demo/tasks/demo-1.md"]]]);
});

test("the panel tells the note window which kind of store it reads", () => {
  const {view} = setup();
  view.data = {store:"berbr", rows:[]};
  view.openNote({note:"/home/dev/.berbr/projects/demo/tasks/demo-1.md", title:"Demo"});
  assert.equal(state.overlays.at(-1).props.vault, false);
  view.data = {store:"vault", rows:[]};
  view.openNote({note:"/notes/demo-1.md", title:"Demo"});
  assert.equal(state.overlays.at(-1).props.vault, true);
  view.data = {rows:[]};
  view.openNote({note:"/notes/demo-1.md", title:"Demo"});
  assert.equal(state.overlays.at(-1).props.vault, false, "an older snapshot without store is no vault");
});

test("picker popups give their result lists a height, since popups size to content", async () => {
  const {pickerHeight} = await import("../forms.js");
  const projects = [{name:"ucp"}, {name:"vpn"}, {name:"kalorie"}];
  const {view, cx} = setup({props:{form:"projects", project:"ucp", projects, overlay:"projects"}});
  const list = byId(view.render(cx), "project-results");
  assert.equal(list.attributes.h[0], pickerHeight(3));
  assert.ok(!list.attributes.flex_1, "a growing list collapses inside a content-sized popup");
  assert.equal(pickerHeight(0), 38);
  assert.equal(pickerHeight(50), 344);
  for (const name of ["ucp", "vpn", "kalorie"]) assert.ok(byId(list, `project-${name}`));
  assert.equal(view.inputs.search.placeholder, "Filter projects");
  const attach = setup({props:{form:"attach-task", ticket:"DEMO-1", tasks:[{id:"a", task:"a", title:"A"}], overlay:"attach"}});
  assert.equal(byId(attach.view.render(attach.cx), "attach-results").attributes.h[0], pickerHeight(1));
});

test("New task: the project picker sets --project, Create more keeps the form and clears it, cmd-enter creates", async () => {
  const tickets = {demo:[{key:"DEMO-1", title:"Login"}], other:[]};
  const {view, cx, jobs} = setup({props:{form:"new", project:"demo", overlay:"new", projects:["demo", "other"], tickets}});
  let root = view.render(cx);
  click(byId(root, "source-jira"), cx);
  root = view.render(cx);
  click(byId(root, "new-ticket-DEMO-1"), cx);
  assert.equal(view.ticket, "DEMO-1");
  click(byId(view.render(cx), "project-choice"), cx);
  click(byId(view.render(cx), "project-other"), cx);
  assert.equal(view.project, "other");
  assert.equal(view.ticket, null, "a ticket of the old project is dropped");
  assert.ok(!byId(view.render(cx), "new-ticket-DEMO-1"));

  const calls = []; view.cli = async args => { calls.push([view.project, ...args]); return "Started"; };
  view.source = "smart"; view.values.title = "First"; view.inputs.title.set_value("First");
  view.createMore = true;
  view.keyEvent({key:"enter", modifiers:{platform:true}}, cx);
  await jobs.shift()(cx);
  assert.deepEqual(calls, [["other", "start", "First", "--bg", "--agent", view.agent]]);
  assert.equal(view.values.title, "");
  assert.ok(!state.calls.some(call => call[0] === "closeOverlay"), "Create more keeps the form open");

  view.createMore = false; view.values.title = "Second";
  view.submitNew(cx); await jobs.shift()(cx);
  assert.deepEqual(state.calls.at(-1), ["closeOverlay", "new"]);
});

test("New task dropdowns use popup content, toggle, dismiss outside, and open one at a time", () => {
  const {view, cx, key} = setup({props:{form:"new", overlay:"new", project:"demo", projects:["demo", "other"]}});
  const focused = [];
  view.choiceFocus = Object.fromEntries(["project", "agent", "list"].map(id => [id, {focus:() => focused.push(id)}]));
  assert.equal(byId(view.render(cx), "project-options"), undefined);
  click(byId(view.render(cx), "project-choice"), cx);
  let root = view.render(cx);
  assert.equal(byId(root, "project-popup").attributes.anchor[0], "bottom_left");
  assert.equal(byId(root, "project-popup").attributes.content[0], byId(root, "project-options"));
  assert.equal(focused.at(-1), "list");
  click(byId(root, "project-choice"), cx);
  assert.equal(view.choosing, null);
  assert.equal(focused.at(-1), "project");
  click(byId(view.render(cx), "project-choice"), cx);
  const oldList = byId(view.render(cx), "project-options");
  click(byId(view.render(cx), "agent-choice"), cx);
  oldList.attributes.on_mouse_down_out[0]({}, cx);
  root = view.render(cx);
  assert.equal(byId(root, "project-options"), undefined);
  assert.ok(byId(root, "agent-options"));
  key("escape");
  assert.equal(view.choosing, null);
  assert.equal(focused.at(-1), "agent");
  assert.deepEqual(state.calls, [], "Escape dismisses the picker first");
  click(byId(view.render(cx), "agent-choice"), cx);
  byId(view.render(cx), "agent-options").attributes.on_mouse_down_out[0]({}, cx);
  assert.equal(view.choosing, null);
  click(byId(view.render(cx), "agent-choice"), cx);
  key("tab"); assert.equal(view.choosing, null);
  click(byId(view.render(cx), "agent-choice"), cx);
  click(byId(view.render(cx), "agent-none"), cx);
  assert.equal(view.agent, "none"); assert.equal(view.choosing, null);
  assert.equal(focused.at(-1), "agent");
  key("escape"); assert.deepEqual(state.calls.at(-1), ["closeOverlay", "new"]);
});

test("New task project dropdown shows all options in a bounded scroll list and keeps keyboard selection", () => {
  for (const projects of [[], ["only"], Array.from({length:23}, (_, index) => `project-${index}`)]) {
    const {view, cx} = setup({props:{form:"new", projects}});
    view.project = null;
    click(byId(view.render(cx), "project-choice"), cx);
    const root = view.render(cx), list = byId(root, "project-options");
    assert.ok(list.attributes.overflow_y_scrollbar);
    assert.equal(list.attributes.h[0], Math.min(Math.max(projects.length, 1), 6) * 30 + 8);
    assert.equal(byId(root, "project-next"), undefined);
    assert.equal(byId(root, "project-previous"), undefined);
    for (const name of projects) assert.ok(byId(list, `project-${name}`));
    if (!projects.length) {
      assert.match(text(root), /No projects available/);
      list.attributes.on_key_down[0]({key:"down"}, cx);
      list.attributes.on_key_down[0]({key:"enter"}, cx);
      assert.equal(view.project, null);
      continue;
    }
    byId(view.render(cx), "project-options").attributes.on_key_down[0]({key:"up"}, cx);
    assert.equal(view.choiceIndex, projects.length - 1);
    byId(view.render(cx), "project-options").attributes.on_key_down[0]({key:"enter"}, cx);
    assert.equal(view.project, projects.at(-1));
    assert.equal(view.choosing, null);
    click(byId(view.render(cx), "project-choice"), cx);
    click(byId(view.render(cx), `project-${projects[0]}`), cx);
    assert.equal(view.project, projects[0]);
  }
});

test("New task close button cancels by mouse or keyboard and is disabled while busy", () => {
  for (const keyboard of [false, true]) {
    const {view, cx} = setup({props:{form:"new", overlay:"new"}});
    const activate = node => keyboard ? node.attributes.on_key_down[0]({key:"enter"}, cx) : click(node, cx);
    activate(byId(view.render(cx), "new-close"));
    assert.deepEqual(state.calls, [["closeOverlay", "new"]]);
    state.calls.length = 0; view.busy = true;
    activate(byId(view.render(cx), "new-close"));
    view.keyEvent({key:"escape"}, cx);
    assert.deepEqual(state.calls, []);
  }
});

test("New task has equal Project and Agent columns, matching popup widths, and picker-specific placeholders", () => {
  const {view, cx} = setup({props:{form:"new", projects:[]}});
  view.project = null; view.agent = null;
  const row = byId(view.render(cx), "new-pickers");
  assert.equal(row.attributes.flex_none.length, 0);
  assert.equal(row.nodes.length, 2);
  assert.ok(byId(row.nodes[0], "project-choice"));
  assert.ok(byId(row.nodes[1], "agent-choice"));
  for (const [index, id, width, placeholder] of [[0, "project", 210, "Choose a project"], [1, "agent", 214, "Choose an agent"]]) {
    assert.ok(row.nodes[index].attributes.flex_1);
    const picker = row.nodes[index].nodes[0];
    assert.ok(picker.attributes.flex_none);
    assert.ok(!picker.attributes.flex_1, "a picker never grows vertically");
    const trigger = byId(view.render(cx), `${id}-choice`);
    assert.match(text(trigger), new RegExp(placeholder));
    trigger.attributes.on_size_change[0]({width, height:34}, cx);
    click(byId(view.render(cx), `${id}-choice`), cx);
    assert.equal(byId(view.render(cx), `${id}-options`).attributes.w[0], width);
    byId(view.render(cx), `${id}-choice`).attributes.on_size_change[0]({width:width - 10, height:34}, cx);
    assert.equal(byId(view.render(cx), `${id}-options`).attributes.w[0], width - 10);
  }
});

test("New task has a compact Smart body, scrolling Jira tickets without paging, and a separate shortcut chip", () => {
  const tickets = Array.from({length:21}, (_, index) => ({key:`DEMO-${index}`, title:"Long title ".repeat(20)}));
  const {view, cx} = setup({props:{form:"new", project:"demo", tickets:{demo:tickets}}});
  view.formOverlay("new");
  assert.equal(state.overlays.at(-1).width, 480);
  assert.equal(state.overlays.at(-1).height, 480);
  let root = view.render(cx);
  assert.ok(!nodes(root).some(node => node.attributes.overflow_y_scrollbar));
  assert.ok(byId(root, "new-body").attributes.flex_none);
  assert.ok(!byId(root, "new-body").attributes.flex_1);
  assert.equal(byId(root, "new-status"), undefined, "no empty status band above the footer");
  assert.equal(byId(root, "create").nodes[0], "Create task");
  assert.ok(nodes(byId(root, "create")).some(node => node.nodes.includes("⌘↵") && node.attributes.border_1));
  click(byId(root, "source-jira"), cx);
  root = view.render(cx);
  const list = byId(root, "new-tickets");
  assert.ok(byId(root, "new-body").attributes.overflow_y_scrollbar);
  assert.ok(list.attributes.overflow_y_scrollbar);
  assert.equal(list.attributes.h[0], 150);
  for (const ticket of tickets) assert.ok(byId(list, `new-ticket-${ticket.key}`));
  assert.equal(byId(root, "new-tickets-previous"), undefined);
  assert.equal(byId(root, "new-tickets-next"), undefined);
  click(byId(root, "new-ticket-DEMO-20"), cx);
  assert.equal(view.ticket, "DEMO-20");
  view.inputs.title.set_value("not found"); view.inputs.title.emit("change", cx);
  root = view.render(cx);
  assert.equal(view.ticket, null);
  assert.match(text(root), /No matching tickets/);
  assert.equal(byId(root, "new-tickets").attributes.h[0], 30);
  click(byId(root, "source-smart"), cx);
  assert.ok(!byId(view.render(cx), "new-body").attributes.overflow_y_scrollbar);
});

test("New task hints stay neutral before submit and validation errors appear only in the error line", () => {
  for (const [source, hint, error] of [["branch", /checked out if it exists/, /Enter a branch/],
    ["jira", /Pick a ticket below or paste a key or link/, /valid Jira ticket key/]]) {
    const {view, cx, jobs} = setup({props:{form:"new", project:"demo"}});
    click(byId(view.render(cx), `source-${source}`), cx);
    let root = view.render(cx);
    assert.match(text(byId(root, "new-hint")), hint);
    assert.doesNotMatch(text(byId(root, "new-hint")), error);
    assert.equal(byId(root, "new-status"), undefined);
    click(byId(root, "create"), cx);
    root = view.render(cx);
    assert.match(text(byId(root, "new-status")), error);
    assert.match(text(byId(root, "new-hint")), hint);
    assert.equal(jobs.length, 0);
  }
});

test("New task tabs preserve text and description, focus the input, and show the compact switches", () => {
  const {view, cx} = setup({props:{form:"new", project:"demo", tickets:{demo:[{key:"DEMO-1", title:"Login"}]}}});
  let focuses = 0; view.inputs.title.focus = () => focuses++;
  view.inputs.title.set_value("Login"); view.inputs.title.emit("change", cx);
  view.inputs.description.set_value("Context\nGoal"); view.inputs.description.emit("change", cx);
  assert.equal(view.worktree, true);
  assert.ok(byId(view.render(cx), "new-worktree"));
  assert.ok(byId(view.render(cx), "new-jira-ticket"));
  click(byId(view.render(cx), "source-jira"), cx);
  assert.equal(byId(view.render(cx), "new-jira-ticket"), undefined);
  click(byId(view.render(cx), "new-ticket-DEMO-1"), cx);
  assert.equal(byId(view.render(cx), "new-ticket-DEMO-1").attributes.bg[0], "selected");
  click(byId(view.render(cx), "source-branch"), cx);
  assert.equal(view.values.title, "Login"); assert.equal(view.inputs.title.value(), "Login");
  assert.equal(view.values.description, "Context\nGoal");
  assert.equal(view.ticket, null); assert.equal(focuses, 2);
  assert.equal(byId(view.render(cx), "advanced"), undefined);
  assert.ok(!("name" in view.inputs)); assert.ok(!("branch" in view.inputs));
  const options = byId(view.render(cx), "new-options");
  assert.ok(byId(options, "new-worktree")); assert.ok(byId(options, "new-jira-ticket"));
  assert.match(text(options), /Worktree.*New Jira ticket/);
  byId(options, "new-worktree").attributes.on_change[0](false, cx);
  assert.equal(view.worktree, false);
});

test("New task Enter submits once, blocks controls while busy, and displays CLI failure without closing", async () => {
  const {view, cx, jobs} = setup({props:{form:"new", project:"demo", overlay:"new", projects:["demo", "other"]}});
  view.values.title = "Task";
  state.execResult = {code:1, stderr:"Cannot create: branch already exists", stdout:""};
  view.inputs.title.emit("submit", cx);
  view.inputs.title.emit("submit", cx);
  view.keyEvent({key:"enter", modifiers:{platform:true}}, cx);
  assert.equal(jobs.length, 1);
  let root = view.render(cx);
  assert.ok(nodes(byId(root, "create")).some(node => node.attributes.spin));
  for (const id of ["project-choice", "source-jira", "agent-choice", "create", "cancel", "new-close"]) {
    const control = byId(root, id);
    assert.equal(control.attributes.tab_stop[0], false, id);
    click(control, cx);
    control.attributes.on_key_down[0]({key:"enter"}, cx);
  }
  for (const node of nodes(root).filter(node => Object.hasOwn(node, "input"))) {
    assert.equal(node.attributes.disabled[0], true);
    assert.equal(node.attributes.tab_stop[0], false);
  }
  for (const id of ["create-more", "new-worktree", "new-jira-ticket"])
    byId(root, id).attributes.on_change[0](id !== "new-worktree", cx);
  assert.equal(view.source, "smart"); assert.equal(view.choosing, null);
  assert.equal(view.worktree, true); assert.equal(view.newTicket, false); assert.equal(view.createMore, false);
  assert.deepEqual(state.calls, []);
  await jobs.shift()(cx);
  assert.equal(view.busy, false);
  assert.match(text(byId(view.render(cx), "new-status")), /Cannot create: branch already exists/);
  assert.ok(!state.calls.some(call => call[0] === "closeOverlay"));
});

test("New task cmd-enter bubbles from keyboard controls to submit instead of activating them", () => {
  const {view, cx, jobs} = setup({props:{form:"new", project:"demo", projects:["demo"]}});
  view.values.title = "Task";
  let stopped = false;
  const context = {...cx, stop_propagation:() => { stopped = true; }};
  const event = {key:"enter", modifiers:{platform:true}};
  for (const id of ["project-choice", "source-jira", "agent-choice", "cancel", "create"]) {
    stopped = false;
    byId(view.render(cx), id).attributes.on_key_down[0](event, context);
    assert.equal(stopped, false, id);
  }
  assert.equal(view.source, "smart");
  assert.deepEqual(state.calls, []);
  view.keyEvent(event, context);
  assert.equal(jobs.length, 1); assert.equal(stopped, true);
});

test("New task Create more clears the title and description, refocuses input, confirms success and sends the chosen project", async () => {
  const {view, cx, jobs} = setup({props:{form:"new", project:"demo", overlay:"new", projects:["demo", "other"]}});
  click(byId(view.render(cx), "project-choice"), cx);
  click(byId(view.render(cx), "project-other"), cx);
  view.createMore = true;
  for (const [key, value] of [["title", "First"], ["description", "Context\nGoal"]]) {
    view.inputs[key].set_value(value); view.inputs[key].emit("change", cx);
  }
  let focuses = 0; view.inputs.title.focus = () => focuses++;
  click(byId(view.render(cx), "create"), cx);
  await jobs.shift()(cx);
  const argv = state.execs[0][1];
  assert.deepEqual(argv.slice(argv.indexOf("--project")), ["--project", "other", "start", "First", "--description", "Context\nGoal", "--bg", "--agent", "codex"]);
  for (const key of ["title", "description"]) {
    assert.equal(view.values[key], ""); assert.equal(view.inputs[key].value(), "");
  }
  assert.equal(focuses, 1); assert.equal(view.ticket, null);
  assert.equal(state.notices.length, 1);
  assert.match(text(byId(view.render(cx), "new-status")), /Task created/);
  assert.ok(!state.calls.some(call => call[0] === "closeOverlay"));
  view.inputs.title.set_value("Second"); view.inputs.title.emit("change", cx);
  assert.doesNotMatch(text(byId(view.render(cx), "new-status")), /Task created/);
});

test("New task description uses four multiline rows; Enter does not submit and cmd-enter names without an agent", async () => {
  const {view, cx, jobs} = setup({props:{form:"new", project:"demo", overlay:"new", agent:"none"}});
  const description = view.inputs.description;
  const textarea = byId(view.render(cx), "description");
  assert.equal(textarea.input, description);
  assert.equal(description.multiline, true);
  assert.equal(description.rows, 4);
  assert.equal(textarea.attributes.h[0], 96);
  assert.equal(description.placeholder, "Describe the task: context, goal, links. Berbr names the task and its branch from it.");
  description.set_value("Context: login fails\nGoal: fix it. https://example.com/issue");
  description.emit("change", cx);
  description.emit("submit", cx);
  view.keyEvent({key:"enter"}, cx);
  assert.equal(jobs.length, 0, "plain Enter in the textarea never submits");
  assert.equal(view.values.description, description.value());
  assert.match(text(byId(view.render(cx), "new-hint")), /small model.*task and branch.*few seconds/);
  description.emit("submit", cx, {secondary:true});
  view.keyEvent({key:"enter", modifiers:{platform:true}}, cx);
  assert.equal(jobs.length, 1, "a secondary textarea submit and its bubbling key create only once");
  const pending = view.render(cx);
  assert.equal(byId(pending, "create").nodes[0], "Naming…");
  assert.ok(nodes(byId(pending, "create")).some(node => node.attributes.spin));
  assert.ok(!byId(pending, "new-body").attributes.overflow_y_scrollbar);
  assert.equal(byId(pending, "description").attributes.disabled[0], true);
  await jobs.shift()(cx);
  const argv = state.execs[0][1];
  assert.deepEqual(argv.slice(argv.indexOf("start")), ["start", "--description", description.value(), "--bg", "--no-agent"]);
  assert.ok(!argv.includes("--unnamed"));
  assert.deepEqual(state.calls.at(-1), ["closeOverlay", "new"]);
});

test("New task can create a ticket from only a description, clears it for Create more and keeps the body fixed", async () => {
  const {view, cx, jobs} = setup({props:{form:"new", project:"demo", overlay:"new"}});
  view.inputs.description.set_value("Context and goal"); view.inputs.description.emit("change", cx);
  byId(view.render(cx), "new-worktree").attributes.on_change[0](false, cx);
  byId(view.render(cx), "new-jira-ticket").attributes.on_change[0](true, cx);
  byId(view.render(cx), "create-more").attributes.on_change[0](true, cx);
  click(byId(view.render(cx), "create"), cx);
  await jobs.shift()(cx);
  const argv = state.execs[0][1];
  assert.deepEqual(argv.slice(argv.indexOf("start")),
    ["start", "--description", "Context and goal", "--bg", "--agent", "codex", "--no-worktree", "--new-ticket"]);
  assert.equal(view.inputs.description.value(), ""); assert.equal(view.values.description, "");
  assert.ok(!state.calls.some(call => call[0] === "closeOverlay"));
  assert.match(text(byId(view.render(cx), "new-status")), /Task created/);
  assert.ok(!byId(view.render(cx), "new-body").attributes.overflow_y_scrollbar);
  click(byId(view.render(cx), "create"), cx);
  assert.match(text(byId(view.render(cx), "new-status")), /Name the task or describe it/);
  assert.ok(!byId(view.render(cx), "new-body").attributes.overflow_y_scrollbar);
  assert.equal(jobs.length, 0);
});
