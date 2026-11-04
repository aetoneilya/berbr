import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { registerHooks } from "node:module";
import { state, theme } from "./support/runtime.mjs";

registerHooks({resolve(specifier, context, next) {
  return ["gpui-kit", "gpui-base", "herdr"].includes(specifier)
    ? {url:new URL("./support/runtime.mjs", import.meta.url).href, shortCircuit:true}
    : next(specifier, context);
}});
const {default:Berbr} = await import("../main.js");
const snapshot = JSON.parse(await readFile(new URL("../fixtures/sample-snapshot.json", import.meta.url)));
const storage = new Map();
globalThis.localStorage = {getItem:key => storage.get(key) ?? null, setItem:(key, value) => storage.set(key, value)};

function setup({data = snapshot, props = {}, saved = null, collapsed = null, liveSpaces = [], keepJobs = false} = {}) {
  storage.clear();
  if (saved) storage.set("berbr.expanded", JSON.stringify(saved));
  if (collapsed) storage.set("berbr.collapsed", JSON.stringify(collapsed));
  Object.assign(state, {props, spaces:liveSpaces, agents:[], shared:new Map(), calls:[], overlays:[], notices:[], execs:[], execResult:null,
    reads:[], readResult:null, readError:null, watches:[], watchers:[], hcCalls:[], hcResult:null, hcError:null, files:null, writes:[], writeError:null, writeResult:null});
  const jobs = [], timers = [];
  const handle = {focused:true, focus() {this.focused = true;}, is_focused() {return this.focused;}};
  const cx = {notify() {}, stop_propagation() {}, focus_handle:() => handle,
    timer:{every:(ms, fn) => timers.push({ms, fn})}, spawn:fn => jobs.push(fn)};
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
  assert.deepEqual(timers.map(timer => timer.ms), [30000, 300000]);
  let refreshed = 0, notified = 0;
  view.refresh = (_cx, cached) => {assert.equal(cached, true); refreshed++;};
  cx.notify = () => notified++;
  const tick = () => timers[0].fn(cx);
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
  view.render(cx);
  assert.equal(jobs.length, 1);
  assert.deepEqual(state.hcCalls, []);
  state.hcResult = {code:0, stdout:"$ npm test\nDo you want to proceed?\n1. Yes", stderr:""};
  await jobs.shift()(cx);
  assert.deepEqual(state.hcCalls, [["read", "address", "--lines", "40"]]);
  assert.ok(text(byId(view.render(cx), "task-blocked")).includes("Asks permission · npm test"));
  view.showDetails(task, cx);
  assert.ok(text(view.render(cx)).includes("Asks permission · npm test"));
  assert.equal(jobs.length, 0);
  agent.seq++;
  view.render(cx);
  state.hcResult.stdout = "Old question?\nWhich branch should I use?\nEnter to send";
  await jobs.shift()(cx);
  assert.ok(text(view.render(cx)).includes("Asks: Which branch should I use?"));
  assert.equal(state.hcCalls.length, 2);
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
  view.render(cx);
  assert.equal(jobs.length, 0);
  view.filter = "";
  view.render(cx);
  state.hcError = new Error("Cannot read terminal");
  await jobs.shift()(cx);
  assert.equal(state.hcCalls.length, 1);
  assert.equal(view.error, null);
  assert.ok(text(view.render(cx)).includes("Waiting for you"));
  assert.equal(jobs.length, 0);
  state.hcError = null;
  state.agents[0].seq++;
  state.hcResult = {code:1, stdout:"Allow command?", stderr:"Failed"};
  view.render(cx);
  await jobs.shift()(cx);
  assert.ok(text(view.render(cx)).includes("Waiting for you"));
  let finish;
  state.agents[0].seq++;
  state.hcResult = new Promise(resolve => {finish = resolve;});
  view.render(cx);
  const pending = jobs.shift()(cx);
  state.agents[0].status = "working"; state.agents[0].seq++;
  view.render(cx);
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
  assert.ok(text(rendered).includes("checkout — loses +2 uncommitted"));
  assert.ok(text(rendered).includes("Closes the Herdr space"));
  assert.ok(!text(rendered).includes("Jira Tickets assigned"));
  const dirty = nodes(rendered).find(node => text(node).includes("checkout — loses +2 uncommitted"));
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

test("expanded rendering wires MR and Review separately and shows live or stopped agents", async () => {
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
  view.cli = async () => {throw new Error("review failed");};
  click(byId(rendered, `${item.id}/review`), cx);
  assert.equal(view.busy, true);
  rendered = view.render(cx);
  const review = byId(rendered, `${item.id}/review`);
  assert.ok(text(review).includes("spinner.svg"));
  assert.equal(review.attributes.opacity[0], 1);
  const other = view.rows().find(row => row.type === "agent");
  assert.equal(byId(rendered, other.id).attributes.opacity[0], 0.4);
  await jobs.shift()(cx);
  assert.equal(view.busy, false);
  assert.equal(view.busyAction, null);
  assert.equal(view.error, "review failed");
  click(byId(view.render(cx), `${item.id}/mr`), cx);
  assert.equal(view.busyAction, `${item.id}/mr`);
  await jobs.shift()(cx);
  assert.deepEqual(state.execs, [["open", [tree.children[0].url]]]);
  assert.equal(state.calls.length, 0);
});

test("inner Enter preserves CLI arguments, focuses agents and opens subtask notes", async () => {
  const tree = {id:"tree", kind:"tree", tree:"branch", task:"work", children:[{id:"agent", kind:"agent", terminal:"pane"}]};
  const subtask = {id:"subtask", kind:"task", note:"/notes/subtask.md"};
  const task = {id:"task", kind:"task", task:"work", children:[tree, subtask]};
  const {view, cx, jobs, key} = setup({data:{rows:[task]}});
  view.showDetails(task, cx);
  const args = [];
  view.cli = async values => {args.push(values); return "";};
  key("down");
  key("enter");
  await jobs.shift()(cx);
  assert.deepEqual(args, [["select", "work", "--tree", "branch"]]);
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
  assert.equal(view.sections().find(section => section.id === "notInTask").expanded, true);
  assert.equal(view.sections().find(section => section.id === "cleanup").expanded, false);
  assert.equal(view.rows().filter(row => row.id?.startsWith("loose:")).length, 3);
  ({view, cx} = setup({collapsed:["inbox", "unsorted"]}));
  assert.deepEqual([...view.collapsed], ["inbox", "cleanup"]);
  assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), ["inbox", "cleanup"]);
  view.toggleSection("cleanup", cx);
  assert.equal(view.sections().find(section => section.id === "cleanup").expanded, true);
  assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), ["inbox"]);
});

test("Not in a task gives a second-line reason; Make task prefills both loose and Inbox tickets", () => {
  const {view, cx} = setup();
  view.collapsed.clear();
  for (const target of ["DEMO-61", "DEMO-44"]) {
    const row = view.rows().find(row => row.target === target);
    view.taskClick(row, cx);
    const rendered = view.render(cx);
    const action = byId(rendered, `${row.id}/loose/action/make-task`);
    assert.equal(text(action), "Make task");
    click(action, cx);
    const props = state.overlays.at(-1).props;
    assert.equal(props.form, "new");
    assert.equal(props.prefill.ticket, target);
    assert.equal(props.prefill.title, row.label.slice(target.length).trim());
    state.props = props;
    const dialog = new Berbr();
    dialog.init({}, cx);
    assert.equal(dialog.ticketMode, "existing");
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
  assert.equal(row.nodes.length, 2);
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


test("worktree clicks only select; Resume is explicit and keeps select arguments", async () => {
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
  assert.deepEqual(state.calls, []);
  const rendered = view.render(cx);
  const line = byId(rendered, item.id);
  const ids = nodes(line).map(node => node.attributes?.id?.[0]);
  assert.ok(ids.indexOf(`${item.id}/resume`) < ids.indexOf(`${item.id}/mr`));
  const resume = byId(rendered, `${item.id}/resume`);
  assert.equal(resume.attributes.tooltip[0], "Resume agent");
  assert.equal(resume.attributes.size[0], 20);
  click(resume, cx);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["select", task.task, "--tree", item.row.tree]]);
});

test("merged worktrees show Remove, and Enter or Remove opens a modal without running CLI", () => {
  const {view, cx, jobs, key} = setup();
  view.showDetails(snapshot.rows[0], cx);
  const item = view.rows().find(row => row.type === "tree" && row.mr?.state === "merged");
  const args = [];
  view.cli = async values => {args.push(values); return "";};
  click(byId(view.render(cx), item.id), cx);
  assert.equal(state.overlays.length, 0);
  const rendered = view.render(cx);
  assert.equal(byId(rendered, `${item.id}/resume`), undefined);
  const remove = byId(rendered, `${item.id}/remove`);
  assert.equal(remove.attributes.tooltip[0], "Remove worktree");
  assert.equal(remove.attributes.size[0], 20);
  click(remove, cx);
  assert.equal(state.overlays.length, 1);
  key("enter");
  assert.equal(state.overlays.length, 2);
  const spec = state.overlays.at(-1);
  assert.equal(spec.props.form, "remove-tree");
  assert.equal(spec.props.tree.tree, item.row.tree);
  assert.equal(spec.width, 440);
  assert.equal(spec.modal, true);
  assert.equal(jobs.length, 0);
  assert.deepEqual(args, []);
});

test("remove confirmation checks current task names, runs done without force and closes on success", async () => {
  const tree = {kind:"tree", tree:"branch"};
  const mr = {kind:"mr", label:"!42 Title", state:"merged"};
  const {view, cx, jobs} = setup({props:{form:"remove-tree", tree, mr, rows:[], overlay:"confirm"}});
  const args = [];
  view.cli = async values => {args.push(values); return values[0] === "app-snapshot" ? JSON.stringify({rows:[]}) : "Removed";};
  const rendered = view.render(cx);
  assert.ok(text(rendered).includes("Remove worktree “branch”?"));
  assert.ok(text(rendered).includes("Its MR !42 is merged."));
  assert.ok(text(rendered).includes("unpushed commits or an open MR"));
  click(byId(rendered, "remove-tree"), cx);
  assert.equal(view.busy, true);
  await jobs.shift()(cx);
  assert.deepEqual(args, [["app-snapshot"], ["done", "branch", "--yes"]]);
  assert.ok(args.every(values => !values.includes("--force")));
  assert.equal(view.busy, false);
  assert.ok(state.calls.some(([command, id]) => command === "closeOverlay" && id === "confirm"));
});

test("remove confirmation blocks known and fresh task-name collisions", async () => {
  const error = "A task has the same name; remove this worktree from the terminal: berbr done --merged";
  for (const cached of [true, false]) {
    const rows = [{kind:"task", task:"different", title:" BRANCH ", done:false}];
    const {view, cx, jobs} = setup({props:{form:"remove-tree", tree:{tree:"branch"}, rows:cached ? rows : []}});
    const args = [];
    view.cli = async values => {args.push(values); return JSON.stringify({rows});};
    click(byId(view.render(cx), "remove-tree"), cx);
    if (!cached) await jobs.shift()(cx);
    assert.equal(view.error, error);
    assert.ok(text(view.render(cx)).includes(error));
    assert.equal(view.busy, false);
    assert.deepEqual(args, cached ? [] : [["app-snapshot"]]);
    assert.equal(state.calls.length, 0);
  }
});

test("remove confirmation fails closed on unavailable snapshots and displays CLI errors", async () => {
  for (const failure of ["snapshot", "invalid", "done"]) {
    const {view, cx, jobs} = setup({props:{form:"remove-tree", tree:{tree:"branch"}, rows:[]}});
    const args = [];
    view.cli = async values => {
      args.push(values);
      if (values[0] === "app-snapshot") {
        if (failure === "snapshot") throw new Error("Snapshot unavailable");
        return JSON.stringify(failure === "invalid" ? {} : {rows:[]});
      }
      throw new Error("Worktree has uncommitted changes");
    };
    view.submitRemoveTree(cx);
    await jobs.shift()(cx);
    assert.equal(view.busy, false);
    assert.equal(view.busyAction, null);
    assert.ok(view.error);
    assert.ok(text(view.render(cx)).includes(view.error));
    assert.equal(args.length, failure === "done" ? 2 : 1);
    assert.equal(state.calls.length, 0);
  }
});

test("Cleanup expands without starting work and offers removal on its inner worktree rows", () => {
  const {view, cx, jobs, key} = setup();
  view.collapsed.delete("cleanup");
  const tree = view.rows().find(row => row.kind === "tree");
  click(byId(view.render(cx), `loose-${tree.id}`), cx);
  assert.equal(view.selected, tree.id);
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
  assert.equal(state.overlays.length, 2);
  view.selected = tree.id;
  key("enter");
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
  const looseRows = view.sections().filter(section => ["cleanup", "notInTask"].includes(section.id)).flatMap(section => section.rows);
  const trees = looseRows.flatMap(row => row.kind === "tree" ? [row] : (row.children ?? []).filter(child => child.kind === "tree"));
  assert.ok(trees.length > 1);
  for (const tree of trees) {
    view.selected = tree.id;
    const actions = view.treeActions(theme(), tree, tree.children?.find(child => child.kind === "mr"), tree.id);
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

test("loose inner trees only offer Remove even when an obsolete task field remains", () => {
  const tree = {id:"loose-tree", kind:"tree", tree:"loose", task:"old task"};
  const {view, cx, jobs} = setup({data:{rows:[tree]}});
  view.collapsed.clear();
  view.taskClick(tree, cx);
  const inner = view.looseItems(tree).find(item => item.type === "tree");
  view.selected = inner.id;
  const rendered = view.render(cx);
  assert.equal(byId(rendered, `${inner.id}/resume`), undefined);
  assert.equal(byId(rendered, `${inner.id}/review`), undefined);
  click(byId(rendered, `${inner.id}/remove`), cx);
  view.enter(view.chosen(), cx);
  assert.equal(state.overlays.length, 2);
  assert.equal(jobs.length, 0);
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

test("loose worktree Resume and inner Enter select the open ticket, never the tree name", async () => {
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
  await jobs.shift()(cx);
  assert.deepEqual(args.at(-1), ["select", "DEMO-61"]);
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
  for (const [label, tone] of [["+1", "working"], ["↑3", "blocked"], ["merged", "dim"], ["35d", "dim"]]) {
    const part = nodes(line).find(node => text(node) === label);
    assert.ok(part, label);
    assert.equal(part.attributes.text_color[0], tone);
  }
  assert.equal(line.nodes[1].attributes.min_w[0], "45%");
  assert.equal(line.nodes[1].attributes.flex_shrink[0], 0);
  assert.equal(line.nodes[2].attributes.flex_shrink[0], 1);
  const state = nodes(line).find(node => node.attributes.tooltip?.[0].startsWith("1 uncommitted change"));
  assert.equal(state.attributes.tooltip[0], "1 uncommitted change · 3 unpushed commits · quiet 35d");
  assert.ok(!text(line).includes("uncommitted"));
  assert.ok(!text(line).includes("unpushed"));
  const header = byId(rendered, `loose-${loose.id}`);
  assert.ok(text(header).includes("merged · worktree left · +1 uncommitted"));
  assert.ok(text(header).includes("↑3 unpushed"));
  view.taskClick(loose, cx);
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
    view.taskClick(row, cx);
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
    props:{form:"note", path:task.note, title:task.title, tickets:[]}}]);
  view.openNote({note:"/notes/other.md", title:"Other"});
  assert.equal(state.overlays.at(-1).key, "berbr-note");
  assert.equal(state.overlays.at(-1).props.path, "/notes/other.md");
  view.openNote({});
  assert.equal(state.overlays.length, 2);
  assert.deepEqual(state.execs, []);
  assert.equal(jobs.length, 0);
});

test("note window loads selectable markdown and wires title bar controls and Escape", async () => {
  const path = "/notes/My task #1.md";
  const {view, cx, jobs, key} = setup({props:{form:"note", path, title:"My task", overlay:"note-window"}});
  assert.ok(text(view.render(cx)).includes("Loading note…"));
  state.readResult = "---\ntitle: Hidden\n---\n# Visible\n\n[[Page|Link]]\n![[image.png]]";
  await jobs.shift()(cx);
  assert.deepEqual(state.reads, [path, "/notes/Page.md"]);
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
  view.cli = async values => {args.push(values); return JSON.stringify(backendPlan({blockers:["MR !7 is open"]}));};
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
  assert.equal(byId(rendered, `${item.id}/resume`), undefined);
  click(byId(rendered, `${item.id}/remove`), cx);
  assert.equal(state.overlays.at(-1).props.form, "remove-tree");
  assert.equal(jobs.length, 0);
});

test("Cleanup offers bulk removal for task-owned trees and confirms backend plans before done --merged", async () => {
  const tree = {id:"tree", kind:"tree", tree:"clean", cwd:"/home/dev/demo/clean", merged:true};
  const {view, cx, jobs} = setup({data:{version:2, rows:[{id:"task", kind:"task", children:[tree, {...tree}]}]}});
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
      {id:"inbox-ticket", kind:"ticket", target:"DEMO-102", label:"Demo inbox", state:"New"},
    ]},
  ]}});
  state.agents = [{address:"blocked", status:"blocked", seq:1}, {address:"working", status:"working", seq:1}];
  context.view.all = true;
  return context;
}

test("every section header toggles and persists, with the specified initial expansion", () => {
  const {view, cx} = sectionPanel();
  const defaults = new Map([["needs", true], ["running", true], ["progress", true], ["ideas", true],
    ["inbox", false], ["notInTask", true], ["cleanup", false], ["done", true]]);
  assert.equal(view.sections().length, defaults.size);
  for (const [id, expanded] of defaults) {
    let section = view.sections().find(section => section.id === id);
    assert.equal(section.expanded, expanded, id);
    let header = byId(view.render(cx), `section-${id}`);
    assert.ok(text(header).includes(`chevron-${expanded ? "down" : "right"}.svg`), id);
    assert.equal(text(header.nodes.at(-1)), "1");
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
  const expected = ["needs", "running", "progress", "ideas", "done", "notInTask", "cleanup", "inbox"];
  let {view} = setup({collapsed:saved});
  assert.deepEqual([...view.collapsed], expected);
  assert.deepEqual(JSON.parse(storage.get("berbr.collapsed")), expected);
  ({view} = setup({collapsed:expected}));
  assert.deepEqual([...view.collapsed], expected);
  ({view} = setup({collapsed:[]}));
  assert.deepEqual([...view.collapsed], []);
  ({view} = setup({collapsed:["inbox", "cleanup"]}));
  assert.deepEqual([...view.collapsed], ["inbox", "cleanup"]);
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

test("Go to space is explicit in task actions and the 20px monitor button stops propagation", () => {
  const task = {id:"space-task", kind:"task", task:"Demo", space:"live", title:"Demo"};
  const {view, cx, jobs} = setup({data:{rows:[task]}, liveSpaces:[{id:"live"}]});
  const action = view.detailItems(task).find(item => item.id === `${task.id}/action/open`);
  assert.equal(action.label, "Go to space");
  assert.equal(view.detailItems(task).some(item => item.label === "Open"), false);
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
  action.action(cx);
  assert.deepEqual(state.calls, [["space.select", {id:"live"}], ["space.select", {id:"live"}]]);
  view.busy = true;
  click(byId(view.render(cx), `task-space-${task.id}`), cx);
  assert.equal(state.calls.length, 2);
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
    for (const mark of ["+2", "↑3", "9d"]) assert.ok(label.includes(mark));
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
