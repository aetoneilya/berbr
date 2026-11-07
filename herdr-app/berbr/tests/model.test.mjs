import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { taskSpace, taskSpaceMatches, walk, groupSections, taskSignals, PanelModel, visibleRows, parseMrFlags,
  normalizeTicketState, isClosedTicket, cleanObsidian, parseFrontmatter, obsidianUri, ticketTitle, taskDetail, looseReason, worktreeNameCollision, cleanCliText, parseTreeMarks, looseWhy,
  agentActivity, formatElapsed, classifyBlocked, closePlan, rowMenu } from "../model.js";

const snapshot = JSON.parse(await readFile(new URL("../fixtures/sample-snapshot.json", import.meta.url)));
const section = (sections, id) => sections.find(item => item.id === id);

test("task menu offers a live space instead of starting work and hides unavailable actions", () => {
  const task = {kind:"task", task:"demo", title:"Demo", note:"/home/dev/demo.md", children:[]};
  const ids = ctx => rowMenu(task, ctx).map(item => item.id);
  assert.deepEqual(ids({spaces:[{id:"space", name:"Demo"}]}), ["space", "rename", "note", "copy-title", "done"]);
  assert.deepEqual(ids({spaces:[]}), ["start", "rename", "note", "copy-title", "done"]);
  assert.ok(!rowMenu({...task, children:[{kind:"tree"}]}).some(item => item.id === "start"));
  assert.ok(!rowMenu({...task, done:true}).some(item => ["start", "done"].includes(item.id)));
  assert.deepEqual(rowMenu({kind:"task", title:"Untitled"}).map(item => item.id), ["copy-title"]);
});

test("active, merged and loose worktrees all offer removal with a danger separator", () => {
  const mr = {kind:"mr", url:"https://gitlab.example/demo/-/merge_requests/1", state:"opened"};
  const tree = {kind:"tree", tree:"demo", task:"task", branch:"demo-branch", cwd:"/home/dev/demo", children:[mr]};
  assert.deepEqual(rowMenu(tree).map(item => item.id), ["resume", "review", "mr", "copy-path", "copy-branch", "remove-tree"]);
  assert.deepEqual(rowMenu(tree).at(-1), {id:"remove-tree", label:"Remove worktree…", danger:true, separatorBefore:true});
  assert.ok(rowMenu({...tree, merged:true}).some(item => item.id === "remove-tree"));
  assert.deepEqual(rowMenu({...tree, task:null}).map(item => item.id), ["mr", "copy-path", "copy-branch", "remove-tree"]);
  assert.deepEqual(rowMenu({kind:"tree", tree:"loose", body:"/home/dev/loose\n +2"}).map(item => item.id), ["copy-path", "remove-tree"]);
  assert.ok(!rowMenu({kind:"tree", tree:"loose"}).some(item => item.id === "copy-path"));
  assert.ok(rowMenu({kind:"tree", tree:"demo"}, {mr}).some(item => item.id === "mr"));
});

test("agent menu offers focus, address and confirmed stop only with a terminal", () => {
  assert.deepEqual(rowMenu({kind:"agent", terminal:"demo"}).map(item => item.id), ["agent", "copy-terminal", "stop-agent"]);
  assert.deepEqual(rowMenu({kind:"agent", terminal:"demo"}).at(-1), {id:"stop-agent", label:"Stop agent…", danger:true, separatorBefore:true});
  assert.equal(rowMenu({kind:"agent"}, {live:{address:"demo"}}).length, 3);
  assert.deepEqual(rowMenu({kind:"agent"}), []);
});

test("MR and ticket menus offer service links and only MRs offer branch copying", () => {
  const fields = {url:"https://example.test/demo", branch:"demo"};
  assert.deepEqual(rowMenu({kind:"mr", ...fields}).map(item => item.label), ["Open in GitLab", "Copy link", "Copy branch"]);
  assert.deepEqual(rowMenu({kind:"ticket", ...fields}).map(item => item.label), ["Open in Jira", "Copy link"]);
  assert.deepEqual(rowMenu({kind:"mr"}), []);
  assert.deepEqual(rowMenu({kind:"mr", branch:"demo"}).map(item => item.id), ["copy-branch"]);
});

test("section headers and presentation detail rows have no menu and inputs stay unchanged", () => {
  for (const row of [null, {kind:"header"}, {kind:"section"}, {kind:"detail", type:"tree"}, {kind:"unknown"}])
    assert.deepEqual(rowMenu(row), []);
  const task = Object.freeze({kind:"task", task:"demo", title:"Demo", children:Object.freeze([])});
  const ctx = Object.freeze({spaces:Object.freeze([{id:"space", name:"Demo"}])});
  assert.deepEqual(rowMenu(task, ctx), rowMenu(task, ctx));
});

test("agent activity strips leading brands and spinners, preserving the actual activity", () => {
  for (const [title, app, expected] of [
    ["  ✳ Claude Code — Fixing tests  ", "claude-code", "Fixing tests"],
    ["⠋ Codex: Reviewing changes", "codex", "Reviewing changes"],
    ["Claude — Reading files", "claude-code", "Reading files"],
    ["✳️ Claude Code — Fixing tests", "claude", "Fixing tests"],
    ["claude-code - Running tests", "Claude Code", "Running tests"],
    ["Fixing Codex tests", "codex", "Fixing Codex tests"],
    ["Codexify tests", "codex", "Codexify tests"],
    ["  Working  ", null, "Working"],
    ["Codex", "codex", ""], ["✳", "claude-code", ""], [null, null, ""],
  ]) assert.equal(agentActivity(title, app), expected);
});

test("agent activity removes only a trailing parent worktree name", () => {
  for (const [title, tree, expected] of [
    ["Read feed task context | demo-branch", "demo-branch", "Read feed task context"],
    ["Read context — DEMO-BRANCH", "demo-branch", "Read context"],
    ["Read context · demo-branch", " DEMO-BRANCH ", "Read context"],
    ["Read context | demo-branch", "feed", "Read context | demo-branch"],
    ["Read context | other", "demo-branch", "Read context | other"],
    ["Read context | demo-branch", null, "Read context | demo-branch"],
    ["Read demo-branch context", "demo-branch", "Read demo-branch context"],
    ["Read context | tree.v2+(a)", "tree.v2+(a)", "Read context"],
    ["Read context | treeXv2a", "tree.v2+(a)", "Read context | treeXv2a"],
    ["Read context | other — demo-branch", "demo-branch", "Read context | other"],
  ]) assert.equal(agentActivity(title, null, tree), expected);
  assert.equal(agentActivity("✳ Claude Code — Reading files | demo-branch", "claude-code", "demo-branch"), "Reading files");
});

test("elapsed time handles missing, invalid, future, minute and hour boundaries", () => {
  for (const [ms, expected] of [[null, ""], [undefined, ""], [NaN, ""], [Infinity, ""],
    [-1000, "0m"], [0, "0m"], [59999, "0m"], [60000, "1m"], [240000, "4m"],
    [3600000, "1h"], [4320000, "1h 12m"], [90000000, "25h"]])
    assert.equal(formatElapsed(ms), expected);
});

test("blocked screens recognize Claude and Codex permission prompts amid noise", () => {
  for (const [screen, detail] of [
    ["Checking tests…\n\n$ npm test\nDo you want to proceed?\n1. Yes\n2. No", "npm test"],
    ["Editing file\n src/main.js\nDo you want to make this edit?\nEsc to cancel", "src/main.js"],
    ["git status\nAllow command?\nApprove once", "git status"],
    ["main.js\nAllow edits to this file?\n1. Yes", "main.js"],
    ["git diff\nApprove\nPress enter to continue", "git diff"],
    ["Run the following command: cargo test\n1. Yes", "cargo test"],
    ["Run the following command\ngit status\nAllow command?", "git status"],
    ["\u001b[32m$ node test.js\u001b[0m\nDo you want to proceed?", "node test.js"],
    ["│ main.js │\n│ Do you want to make this edit? │", "main.js"],
  ]) assert.deepEqual(classifyBlocked(screen), {kind:"permission", detail});
  assert.deepEqual(classifyBlocked("Unrelated progress message\nDo you want to proceed?"), {kind:"permission"});
  assert.deepEqual(classifyBlocked("Run the following command:"), {kind:"permission"});
  const command = "git " + "x".repeat(100);
  assert.equal(classifyBlocked(`${command}\nAllow command?`).detail, command.slice(0, 80));
});

test("blocked screens pick the last question and otherwise wait", () => {
  assert.deepEqual(classifyBlocked("Old question?\nWorking…\nWhich branch should I use?\n\nEnter to send"),
    {kind:"question", detail:"Which branch should I use?"});
  assert.deepEqual(classifyBlocked("Which approach do you approve?"), {kind:"question", detail:"Which approach do you approve?"});
  for (const text of [null, "", "\n\n", "Working…\nPress enter to continue"])
    assert.deepEqual(classifyBlocked(text), {kind:"waiting"});
});

test("demo close plan lists its open MR, five worktrees, open ticket and merged proof", () => {
  const task = snapshot.rows.find(row => row.task === "Demo refactor");
  const before = JSON.stringify(task);
  const plan = closePlan(task);
  assert.deepEqual(plan.blockers, ["MR !122 is open"]);
  assert.equal(plan.removals.length, 5);
  assert.ok(plan.removals.every(tree => tree.changes === 0 && tree.unpushed === 0));
  assert.deepEqual(plan.jira, ["DEMO-45"]);
  assert.deepEqual(plan.also, ["Marks the note done"]);
  assert.equal(plan.proof.count, 6);
  assert.ok(plan.proof.latest.label.startsWith("!139"));
  assert.equal(JSON.stringify(task), before);
});

test("close plan finds deep blockers, deduplicates references and marks lost changes", () => {
  const mr = {id:"mr", kind:"mr", label:"!7 Deep MR", state:"opened", url:"https://gitlab/mr/7"};
  const tree = {id:"tree", kind:"tree", tree:"checkout", body:"/checkout\n +2 ↑3", children:[mr]};
  const task = {space:"live", children:[tree, {kind:"task", children:[mr, {...tree},
    {kind:"ticket", target:"KEY-1", state:"In progress"},
    {kind:"ticket", target:"KEY-1", state:"In review"},
    {kind:"ticket", target:"KEY-2", state:"Committed"},
    {kind:"ticket", target:"KEY-3", state:"Closed"}]}]};
  const plan = closePlan(task, [{id:"live"}]);
  assert.deepEqual(plan.blockers, ["MR !7 is open", "checkout: ↑3 unpushed"]);
  assert.equal(plan.removals.length, 1);
  assert.equal(plan.removals[0].changes, 2);
  assert.deepEqual(plan.jira, ["KEY-1"]);
  assert.deepEqual(plan.also, ["Marks the note done", "Closes the Herdr space"]);
  assert.deepEqual(plan.proof, {count:0, latest:null});
  const empty = closePlan(null);
  for (const key of ["blockers", "removals", "jira"]) assert.deepEqual(empty[key], []);
  assert.deepEqual(closePlan({space:"gone"}, [{id:"other"}]).also, ["Marks the note done"]);
});

test("fixture tasks and header-delimited work are all grouped", () => {
  const sections = groupSections(snapshot.rows);
  assert.equal(section(sections, "progress").rows.filter(row => row.kind === "task").length, 9);
  assert.equal(section(sections, "todo").rows.length, 4);
  assert.equal(section(sections, "todo").rows[0].state, "idea");
  assert.equal(section(sections, "needs").rows.length, 0);
  assert.equal(section(sections, "running").rows.length, 0);
  assert.equal(section(sections, "done").rows.length, 19);
  assert.deepEqual(section(sections, "todo").rows.filter(row => row.kind === "ticket").map(row => row.target), ["DEMO-44", "DEMO-43", "DEMO-36"]);
  const start = snapshot.rows.findIndex(row => row.label === "no task");
  const end = snapshot.rows.findIndex(row => row.label === "assigned to me in Jira");
  const loose = snapshot.rows.slice(start + 1, end);
  assert.deepEqual(section(sections, "progress").rows.filter(row => row.kind !== "task"), loose.filter(row => row.kind === "ticket" && !isClosedTicket(row)));
  assert.deepEqual(section(sections, "cleanup").rows, loose.filter(row => row.kind !== "ticket" || isClosedTicket(row)));
});

test("Todo keeps idea tasks before assigned tickets in each part's original order", () => {
  const first = {id:"first", kind:"task", state:"idea"};
  const second = {id:"second", kind:"task", state:"idea"};
  const ticket = {id:"ticket", kind:"ticket", target:"DEMO-1"};
  const sibling = {id:"sibling", kind:"ticket", target:"DEMO-2"};
  const sections = groupSections([
    {kind:"header", label:"assigned to me in Jira", children:[ticket]},
    first, sibling, second, ticket,
  ]);
  assert.deepEqual(section(sections, "todo").rows, [first, second, ticket, sibling]);
  assert.equal(section(sections, "todo").label, "Todo");
  assert.deepEqual(section(groupSections([first]), "todo").rows, [first]);
  assert.deepEqual(section(groupSections([
    {kind:"header", label:"assigned to me in Jira", children:[ticket]},
  ]), "todo").rows, [ticket]);
});

test("subtasks stay out of panel sections while direct header tasks are included", () => {
  const grandchild = {id:"grandchild", kind:"task", done:true};
  const child = {id:"child", kind:"task", done:true, children:[grandchild]};
  const parent = {id:"parent", kind:"task", done:true, children:[child]};
  const headerTask = {id:"header-task", kind:"task", state:"active", children:[child]};
  const sections = groupSections([parent, {kind:"header", children:[headerTask]}]);
  assert.deepEqual(section(sections, "done").rows, [parent]);
  assert.deepEqual(section(sections, "progress").rows, [headerTask]);
  assert.ok(sections.every(item => !item.rows.includes(child) && !item.rows.includes(grandchild)));
});

test("open descendants of done ancestors stay visible exactly once, including blocked work", () => {
  const child = {id:"task:demo-1", kind:"task", task:"demo-1", title:"Open child", done:false, state:"active",
    children:[{kind:"agent", terminal:"child-agent"}]};
  const parent = {id:"task:demo-0", kind:"task", done:true,
    children:[{kind:"task", id:"done-child", done:true, children:[child]}]};
  const panel = new PanelModel().update({rows:[parent, child]}, [{address:"child-agent", status:"blocked"}], [], {showDone:false});
  assert.deepEqual(panel.rows, [child]);
  assert.equal(panel.visible[0].id, "needs");
  assert.ok(!panel.visible.some(section => section.id === "done"));
});

test("one agent change invalidates its owning tasks and ancestors, retaining unrelated caches", () => {
  const first = {kind:"task", id:"first", children:[{kind:"agent", terminal:"pane-a"}]};
  const other = {kind:"task", id:"other", children:[{kind:"agent", terminal:"pane-b"}]};
  const parent = {kind:"task", id:"parent", children:[first]};
  const data = {rows:[parent, other]}, live = [{address:"agent", pane:"pane-a", seq:1, status:"working"}];
  const panel = new PanelModel().update(data, live);
  const untouched = [panel.signal(other), panel.detail(other), panel.plan(other)];
  const before = [panel.signal(parent), panel.signal(first), panel.detail(first)];
  live[0].seq++; live[0].status = "blocked";
  panel.update(data, live);
  assert.notEqual(panel.signal(parent), before[0]);
  assert.notEqual(panel.signal(first), before[1]);
  assert.notEqual(panel.detail(first), before[2]);
  assert.deepEqual([panel.signal(other), panel.detail(other), panel.plan(other)], untouched);
  assert.equal(panel.signal(other), untouched[0]);
  assert.equal(panel.detail(other), untouched[1]);
  // In-place movement must invalidate both the old and the new owner.
  live[0].pane = "pane-b"; live[0].seq++;
  panel.update(data, live);
  assert.equal(panel.signal(parent).agents.length, 0);
  assert.equal(panel.signal(other).status, "blocked");
  panel.update(data, []);
  assert.equal(panel.signal(other).agents.length, 0);
});

test("nested headers, missing labels and unknown kinds are safe", () => {
  const ticket = {id:"ticket", kind:"ticket", label:"Ticket"};
  const tree = {id:"tree", kind:"tree", label:"Tree"};
  const unknown = {id:"future", kind:"future"};
  const sections = groupSections([
    {id:"assigned", kind:"header", label:"Assigned TO ME IN JIRA", children:[ticket]},
    {id:"unknown", kind:"header", children:[tree, unknown]},
  ]);
  assert.deepEqual(section(sections, "todo").rows, [ticket]);
  assert.deepEqual(section(sections, "cleanup").rows, [tree, unknown]);
  assert.deepEqual(walk(null), []);
  assert.ok(groupSections().every(item => item.rows.length === 0));
});

test("live agents move tasks into attention or running groups; stale agents do not", () => {
  const task = { ...snapshot.rows[0], children:[...snapshot.rows[0].children,
    {id:"agent", kind:"agent", terminal:"terminal-1"}] };
  const rows = [task, ...snapshot.rows.slice(1)];
  for (const status of ["blocked", "done"]) {
    const sections = groupSections(rows, [{address:"terminal-1", status}]);
    assert.equal(section(sections, "needs").rows[0].id, task.id);
    assert.equal(section(sections, "progress").rows.filter(row => row.kind === "task").length, 8);
  }
  assert.equal(section(groupSections(rows, [{address:"terminal-1", status:"working"}]), "running").rows[0].id, task.id);
  assert.equal(section(groupSections(rows, [{address:"other", status:"blocked"}]), "progress").rows.filter(row => row.kind === "task").length, 9);
  const done = { ...task, done:true };
  assert.equal(section(groupSections([done], [{address:"terminal-1", status:"blocked"}]), "done").rows[0], done);
});

test("attention wins over working, and duplicate terminal references count once", () => {
  const task = {children:[{kind:"agent", terminal:"a"}, {kind:"agent", terminal:"a"}, {kind:"agent", terminal:"b"}]};
  const signal = taskSignals(task, [{address:"a", status:"working"}, {address:"b", status:"blocked"}]);
  assert.equal(signal.status, "blocked");
  assert.equal(signal.attention, "Needs you");
  assert.equal(signal.working, 1);
  assert.equal(taskSignals(task, [{address:"a", status:"done"}]).attention, "Finished");
});

test("MR status tokens, age, and worst-flag precedence", () => {
  assert.deepEqual(parseMrFlags(" 27d ci…"), {ci:"running", conflict:false, draft:false, age:27, worst:null});
  assert.deepEqual(parseMrFlags(" 163d ci… conflict"), {ci:"running", conflict:true, draft:false, age:163, worst:"conflict"});
  assert.deepEqual(parseMrFlags(" draft 90d ci✗"), {ci:"failed", conflict:false, draft:true, age:90, worst:"CI failed"});
  assert.deepEqual(parseMrFlags(""), {ci:null, conflict:false, draft:false, age:null, worst:null});
  assert.equal(parseMrFlags("draft conflict ci✗").worst, "CI failed");
  assert.equal(parseMrFlags("draft conflict ci✓").worst, "conflict");
  assert.equal(parseMrFlags("draft").worst, "draft");
  assert.equal(parseMrFlags("0d ci✓").age, 0);
  assert.equal(parseMrFlags("ci✓").ci, "passed");
  assert.equal(parseMrFlags(null).worst, null);
  assert.equal(parseMrFlags("not-conflict undrafted ci✗suffix 3days").worst, null);
});

test("task signals expose the first open ticket, deduplicated open MRs and live spaces", () => {
  const first = snapshot.rows[0];
  const signal = taskSignals(first);
  assert.equal(signal.ticket.target, "DEMO-45");
  assert.equal(signal.mrKey, "!122");
  assert.equal(signal.trees.length, 5);
  const mr = {id:"mr", kind:"mr", label:"!42 Hello", state:"opened", body:"ci✗"};
  const task = {space:"s", children:[mr, {kind:"tree", children:[mr]}, {...mr, id:"merged", state:"merged"}]};
  const deduped = taskSignals(task, [], [{id:"s", active:true}]);
  assert.equal(deduped.mrs.length, 1);
  assert.equal(deduped.mrFlag, "CI failed");
  assert.equal(deduped.liveSpace.active, true);
  assert.equal(taskSignals({space:"gone"}).liveSpace, undefined);
});

test("ticket and MR states are normalized", () => {
  for (const [values, expected] of [
    [["Новый", "new", "OPEN", "To Do"], "New"],
    [["В работе", "IN PROGRESS"], "In progress"],
    [["ON REVIEW", "In review", "Review"], "In review"],
    [["Commited", "Committed"], "Committed"],
    [["Закрыт", "Closed", "Done", "Resolved"], "Closed"],
  ]) for (const value of values) assert.equal(normalizeTicketState(value), expected);
  assert.equal(normalizeTicketState("  WAITING FOR INPUT  "), "Waiting for input");
  assert.equal(normalizeTicketState(null), "");
  assert.ok(isClosedTicket({state:"Commited"}));
  assert.ok(!isClosedTicket({state:null}));
});

test("Obsidian embeds are removed and wikilinks become readable text", () => {
  assert.equal(cleanObsidian("Intro\n![[x.excalidraw]]\n[[Demo task 04]], [[a|b]], [[a#h]]."), "Intro\nDemo task 04, b, a.");
  assert.equal(cleanObsidian("  ![[x.excalidraw]]  \n"), "");
  assert.equal(cleanObsidian("Before ![[x]] after [[a#h|Alias]]"), "Before  after Alias");
  assert.equal(cleanObsidian("# Heading\n\n- **Markdown** `code`"), "# Heading\n\n- **Markdown** `code`");
  assert.equal(cleanObsidian(null), "");
});

test("filter matches titles, task keys, tags and every descendant label, branch and ticket target", () => {
  const task = {kind:"task", id:"task", title:"Readable title", task:"internal-key", tags:["feature"], children:[
    {kind:"tree", label:"Checkout", branch:"dev/branch", children:[{kind:"ticket", target:"DEMO-123"}]},
  ]};
  for (const query of ["TITLE", "internal-key", "feature", "checkout", "DEV/branch", "demo-123", " "])
    assert.ok(new PanelModel().update({rows:[task]}, [], [], {query}).rows.includes(task), query);
  assert.deepEqual(new PanelModel().update({rows:[task]}, [], [], {query:"missing"}).rows, []);
  assert.deepEqual(new PanelModel().update({rows:[{kind:"task", children:null}]}, [], [], {query:"anything"}).rows, []);
  assert.ok(new PanelModel().update(snapshot, [], [], {query:"demo-checkout-06"}).rows.length);
});

test("visible rows skip collapsed sections, expand during search and honor Done", () => {
  const data = snapshot;
  const collapsed = new Set(["todo", "cleanup"]);
  const visible = new PanelModel().update(data, [], [], {collapsed}).visible;
  assert.equal(visibleRows(visible).length, 12);
  assert.ok(!visible.some(item => item.id === "done"));
  const filtered = new PanelModel().update(data, [], [], {collapsed, query:"demo-44"}).visible;
  assert.equal(filtered[0].id, "todo");
  assert.equal(filtered[0].expanded, true);
  assert.equal(visibleRows(filtered)[0].target, "DEMO-44");
  assert.equal(visibleRows(new PanelModel().update(data, [], [], {collapsed, showDone:true}).visible).length, 31);
  assert.deepEqual(new PanelModel().update(data, [], [], {query:"no-such-task"}).visible, []);
});

test("Assigned ticket title prefills remove only the leading ticket key", () => {
  assert.equal(ticketTitle({target:"DEMO-123", label:"DEMO-123  Fix DEMO-123"}), "Fix DEMO-123");
  assert.equal(ticketTitle({target:"DEMO-123", label:"DEMO-123"}), "");
  assert.equal(ticketTitle({label:"No key"}), "No key");
});


test("demo details keep tickets as attributes and MRs with their worktrees", () => {
  const task = snapshot.rows.find(row => row.task === "Demo refactor");
  const detail = taskDetail(task);
  assert.deepEqual(detail.tickets.open.map(row => row.target), ["DEMO-45"]);
  assert.deepEqual(detail.tickets.closed.map(row => row.target), ["DEMO-47", "DEMO-48"]);
  assert.equal(detail.trees.length, 5);
  assert.deepEqual(detail.trees.map(tree => tree.row.tree), [
    "demo-checkout-03", "demo-checkout-07", "demo-checkout-02", "demo-checkout-05", "demo-checkout-06",
  ]);
  const config = detail.trees.find(tree => tree.row.tree === "demo-checkout-03");
  assert.ok(config.mr.label.startsWith("!122 "));
  assert.equal(config.mr.state, "opened");
  assert.equal(detail.trees.find(tree => tree.row.tree === "demo-checkout-07").mr, null);
  assert.deepEqual(detail.history.map(item => item.row.label.match(/!\d+/)[0]), ["!133", "!131", "!121", "!120"]);
  assert.ok(detail.history.every(item => item.tickets.length === 1 && item.tickets[0] === "DEMO-47"));
  assert.equal(new Set(detail.history.map(item => item.row.id)).size, 4);
  assert.deepEqual(detail.openMrs, []);
  assert.deepEqual(detail.mainAgents, []);
  assert.ok(!detail.excerpt.includes("`"));
});

test("task details match live agents, deduplicate loose MRs and exclude subtask descendants", () => {
  const merged = {id:"mr:1", kind:"mr", state:"merged", url:"https://git/mr/1"};
  const open = {id:"mr:2", kind:"mr", state:"opened"};
  const treeMr = {id:"mr:3", kind:"mr", state:"opened"};
  const agent = {id:"agent", kind:"agent", terminal:"pane", state:"idle"};
  const main = {id:"main", kind:"agent", terminal:"terminal", state:"done"};
  const subtask = {id:"subtask", kind:"task", children:[{kind:"tree"}, {kind:"ticket", target:"OTHER"}, open]};
  const task = {body:"\n![[drawing]]\n\n## **First** _paragraph_ with `candidates_providers.anchor_to_item` and [[Note|alias]].\n- *More* [text](https://example.com).\n\nSecond paragraph.", children:[
    {id:"tree", kind:"tree", children:[treeMr, agent]}, main,
    {kind:"ticket", target:"ONE", state:"New", children:[merged, open, treeMr]},
    {kind:"ticket", target:"TWO", state:"Closed", children:[{...merged, id:"another-id"}, merged]},
    merged, subtask,
  ]};
  const before = structuredClone(task);
  const live = {address:"different-address", pane:"pane", status:"working", title:"Live title", program:{name:"Codex"}};
  const detail = taskDetail(task, [live]);
  assert.equal(detail.trees[0].agents[0].row, agent);
  assert.equal(detail.trees[0].agents[0].live, live);
  assert.equal(detail.trees[0].agents[0].status, "working");
  assert.equal(detail.mainAgents[0].row, main);
  assert.equal(detail.mainAgents[0].live, undefined);
  assert.equal(detail.mainAgents[0].status, "done");
  assert.deepEqual(detail.history, [{row:merged, tickets:["ONE", "TWO"]}]);
  assert.deepEqual(detail.openMrs, [{row:open, tickets:["ONE"]}]);
  assert.deepEqual(detail.subtasks, [subtask]);
  assert.equal(detail.trees.length, 1);
  assert.equal(detail.excerpt, "First paragraph with candidates_providers.anchor_to_item and alias. More text.");
  assert.deepEqual(task, before);
  assert.equal(taskDetail(task, [{address:"terminal", status:"blocked"}]).mainAgents[0].status, "blocked");
  assert.equal(taskSignals(task, [live]).working, 1);
});

test("task details handle empty content, direct MRs and bounded plain excerpts", () => {
  const empty = {tickets:{open:[], closed:[]}, trees:[], mainAgents:[], openMrs:[], history:[], subtasks:[], excerpt:""};
  assert.deepEqual(taskDetail(), empty);
  assert.deepEqual(taskDetail({body:null, children:null}), empty);
  assert.equal(taskDetail({body:"![[drawing]]\n\n- **Bold** and `code`"}).excerpt, "Bold and code");
  const long = taskDetail({body:"x".repeat(300) + "\n\nignored"}).excerpt;
  assert.equal(long.length, 240);
  assert.ok(long.endsWith("…"));
  const mr = {id:"direct", kind:"mr", state:"OPENED"};
  const closed = {id:"closed", kind:"mr", state:"CLOSED"};
  assert.deepEqual(taskDetail({children:[mr, closed]}).openMrs, [{row:mr, tickets:[]}]);
  assert.deepEqual(taskDetail({children:[mr, closed]}).history, [{row:closed, tickets:[]}]);
  const unmatched = taskDetail({children:[{kind:"agent", state:"working"}]}, [{status:"blocked"}]);
  assert.equal(unmatched.mainAgents[0].live, undefined);
});


test("loose fixture work splits into active tickets and explained cleanup", () => {
  const sections = groupSections(snapshot.rows);
  const active = section(sections, "progress").rows.filter(row => row.kind !== "task");
  assert.deepEqual(active.map(row => [row.target, looseReason(row)]), [
    ["DEMO-61", "merged · worktree left"],
    ["DEMO-42", "worktree demo-checkout-11"],
    ["DEMO-10", "MR !123 open"],
  ]);
  const cleanup = section(sections, "cleanup").rows;
  assert.equal(cleanup.length, 10);
  assert.deepEqual(cleanup.map(row => [row.target ?? (row.kind === "tree" ? row.tree : row.label.match(/!\d+/)[0]), looseReason(row)]), [
    ["DEMO-54", "merged · worktree left"],
    ["DEMO-38", "ticket closed · 2 MRs open"],
    ["DEMO-30", "ticket closed · 1 MR open"],
    ["demo-checkout-01", "no ticket"],
    ["!109", "no ticket · 90d"], ["!106", "no ticket · 100d"],
    ["!105", "no ticket · 122d"], ["!104", "no ticket · 122d"],
    ["!101", "no ticket · 170d"], ["!100", "no ticket · 190d"],
  ]);
  assert.equal(cleanup.filter(row => row.kind === "mr").length, 6);
  assert.ok(!sections.some(item => item.id === "unsorted"));
});

test("unknown tickets stay active; cleanup reasons handle both leftovers, zero MRs and unknown ages", () => {
  const unknown = {id:"unknown", kind:"ticket", state:null};
  const closed = {id:"closed", kind:"ticket", state:"Committed", children:[
    {kind:"tree", state:"active"}, {id:"mr", kind:"mr", state:"opened"},
  ]};
  const sections = groupSections([{kind:"header", label:"no task", children:[unknown, closed]}]);
  assert.deepEqual(section(sections, "progress").rows, [unknown]);
  assert.deepEqual(section(sections, "cleanup").rows, [closed]);
  assert.equal(looseReason(unknown), "ticket open");
  assert.equal(looseReason(closed), "ticket closed · worktree left · 1 MR open");
  assert.equal(looseReason({...closed, children:[...closed.children,
    {kind:"tree", state:"active", children:[closed.children[1]]},
  ]}), "ticket closed · worktree left · 1 MR open");
  assert.equal(looseReason({...closed, children:[]}), "ticket closed");
  assert.equal(looseReason({...closed, children:[{kind:"tree", state:"removed"}]}), "ticket closed");
  assert.equal(looseReason({kind:"mr", body:null}), "no ticket");
  assert.equal(looseReason({kind:"mr", body:"0d"}), "no ticket · 0d");
});

test("worktrees sort by live agents, open MR, no MR and closed MR, then name without mutating input", () => {
  const tree = (name, state, terminal) => ({kind:"tree", label:name, children:[
    ...(state ? [{kind:"mr", state}] : []), ...(terminal ? [{kind:"agent", terminal, state:"working"}] : []),
  ]});
  const task = {children:[tree("z-closed", "closed"), tree("z-empty"), tree("z-open", "opened"),
    tree("a-empty", null, "stale"), tree("b-live", "merged", "pane"), tree("a-live", null, "address"),
    tree("a-open", "opened"), tree("a-merged", "merged")]};
  const before = structuredClone(task);
  const detail = taskDetail(task, [{pane:"pane", status:"idle"}, {address:"address", status:"blocked"}]);
  assert.deepEqual(detail.trees.map(item => item.row.label), [
    "a-live", "b-live", "a-open", "z-open", "a-empty", "z-empty", "a-merged", "z-closed",
  ]);
  assert.deepEqual(task, before);
});


test("worktree removal detects open task names and titles, ignoring case and outer spaces", () => {
  const rows = [{kind:"header", children:[
    {kind:"task", task:"  Branch  ", title:"Another title", done:false},
    {kind:"task", task:"other", title:" Display name ", children:[{kind:"task", task:"nested"}]},
    {kind:"task", task:"finished", title:"Old title", done:true},
    {kind:"tree", tree:"not-a-task", title:"not-a-task"},
  ]}];
  const before = structuredClone(rows);
  for (const name of ["branch", " BRANCH ", "another TITLE", "display NAME", "nested"])
    assert.equal(worktreeNameCollision(rows, name), true, name);
  for (const name of ["finished", "old title", "not-a-task", "absent", "", null])
    assert.equal(worktreeNameCollision(rows, name), false, name);
  assert.equal(worktreeNameCollision(null, "branch"), false);
  assert.deepEqual(rows, before);
});


test("CLI error text removes repeated zsh zle noise without changing Berbr's message", () => {
  const message = 'Error: no task or ticket "demo-checkout-01"';
  const noise = "(eval):1: can't change option: zle";
  assert.equal(cleanCliText(noise + noise + message), message);
  assert.equal(cleanCliText("  " + noise + "\n" + message + "\n" + noise + "  "), message);
  assert.equal(cleanCliText("(eval):42: can't change option: zle"), "");
  assert.equal(cleanCliText(message), message);
  assert.equal(cleanCliText(null), "");
});


test("worktree marks parse paths, changes, unpushed commits, merge state and inactivity", () => {
  const empty = {path:"", changes:0, unpushed:0, merged:false, quietDays:null};
  assert.deepEqual(parseTreeMarks(" +1 ↑3"), {...empty, changes:1, unpushed:3});
  assert.deepEqual(parseTreeMarks(" +1 ↑3\n"), {...empty, changes:1, unpushed:3});
  assert.deepEqual(parseTreeMarks(" ✓ merged"), {...empty, merged:true});
  assert.deepEqual(parseTreeMarks(" quiet 35d"), {...empty, quietDays:35});
  assert.deepEqual(parseTreeMarks(""), empty);
  assert.deepEqual(parseTreeMarks(null), empty);
  assert.deepEqual(parseTreeMarks("/repo/checkout\n +12 ↑30 ✓ merged quiet 0d"), {
    path:"/repo/checkout", changes:12, unpushed:30, merged:true, quietDays:0,
  });
  assert.deepEqual(parseTreeMarks("/repo/quiet 35d\n", "finished"), {...empty, path:"/repo/quiet 35d", merged:true});
  assert.deepEqual(parseTreeMarks("/repo/checkout"), {...empty, path:"/repo/checkout"});
  assert.deepEqual(parseTreeMarks("/repo\n +0 ↑0 quietly 35d"), {...empty, path:"/repo"});
});

test("loose why explains tree, MR, assigned ticket, closed ticket and unlinked work", () => {
  const tree = {kind:"tree", tree:"checkout", branch:"dev/KEY-1"};
  const mr = {kind:"mr", label:"!42 Title", state:"opened"};
  const ticket = {kind:"ticket", target:"KEY-1", state:"New", children:[tree, mr]};
  assert.equal(looseWhy(ticket), "Your worktree checkout is on branch dev/KEY-1, which mentions KEY-1.");
  assert.equal(looseWhy({...ticket, children:[mr]}), "Your open MR !42 mentions KEY-1.");
  assert.equal(looseWhy({...ticket, children:[]}), "Assigned to you in Jira; nothing started yet.");
  assert.equal(looseWhy({...ticket, state:"Closed"}), "The ticket is closed, but your worktree checkout is on branch dev/KEY-1, which mentions KEY-1.");
  assert.equal(looseWhy({...ticket, state:"Committed", children:[mr]}), "The ticket is closed, but your open MR !42 mentions KEY-1.");
  assert.equal(looseWhy({...ticket, state:"Closed", children:[]}), "The ticket is closed; no worktree or open MR remains.");
  assert.equal(looseWhy(mr), "Your open MR has no ticket key in its branch or title.");
  assert.equal(looseWhy(tree), "This worktree's branch matches no ticket or task.");
});

test("loose why identifies a ticket key in the branch before considering the MR title", () => {
  const tree = {kind:"tree", tree:"checkout", branch:"dev/key-1", children:[{kind:"mr", label:"!42 KEY-1 Title"}]};
  const ticket = {kind:"ticket", target:"KEY-1", children:[tree]};
  assert.equal(looseWhy(ticket), "Your worktree checkout is on branch dev/key-1, which mentions KEY-1.");
  assert.equal(looseWhy({...ticket, state:"Closed"}), "The ticket is closed, but your worktree checkout is on branch dev/key-1, which mentions KEY-1.");
});

test("loose why identifies a ticket key in its worktree MR title regardless of MR state", () => {
  const tree = {kind:"tree", tree:"checkout", branch:"dev/checkout", children:[
    {kind:"mr", label:"!41 Other ticket", state:"opened"},
    {kind:"mr", label:"!42 Fix key-1", state:"merged"},
  ]};
  const ticket = {kind:"ticket", target:"KEY-1", children:[tree]};
  assert.equal(looseWhy(ticket), "The branch is merged; its worktree is still on disk.");
  assert.equal(looseWhy({...ticket, state:"Committed"}), "The branch is merged; its worktree is still on disk.");
});

test("loose why uses a neutral link when neither branch nor that tree's MR title contains the key", () => {
  const tree = {kind:"tree", tree:"checkout", children:[{kind:"mr", label:"!42 Other ticket"}]};
  const ticket = {kind:"ticket", target:"KEY-1", children:[tree, {kind:"mr", label:"!43 KEY-1", state:"opened"}]};
  assert.equal(looseWhy(ticket), "Your worktree checkout is linked to KEY-1.");
  assert.equal(looseWhy({...ticket, state:"Closed"}), "The ticket is closed, but your worktree checkout is linked to KEY-1.");
  assert.equal(looseWhy({...ticket, children:[{...tree, children:null}]}), "Your worktree checkout is linked to KEY-1.");
});

test("frontmatter is stripped only when a leading block is terminated", () => {
  for (const [source, expected] of [
    ["---\ntitle: Task\ntags: [work]\n---\n# Body", "# Body"],
    ["---\r\ntitle: Task\r\n---\r\nBody", "Body"],
    ["---\n---\nBody", "Body"],
    ["---\n---\nBody\n---", "Body\n---"],
    ["---\ntitle: Task\n---", ""],
    ["# Body\n\n---\ntext\n---", "# Body\n\n---\ntext\n---"],
    ["---\ntitle: Task\nBody", "---\ntitle: Task\nBody"],
    ["---\ntitle: Task\n---suffix\nBody", "---\ntitle: Task\n---suffix\nBody"],
    ["Body", "Body"], ["", ""], [null, ""],
  ]) assert.equal(parseFrontmatter(source).body, expected);
});

test("Obsidian URIs encode the complete path as a query value", () => {
  assert.equal(obsidianUri("/notes/My task.md"), "obsidian://open?path=%2Fnotes%2FMy%20task.md");
  const path = "/Заметки/task #1 & 2?.md";
  assert.equal(new URL(obsidianUri(path)).searchParams.get("path"), path);
  assert.equal(obsidianUri(""), "obsidian://open?path=");
});

test("snapshot merge metadata drives worktree state, reasons and safe bulk fallback", async () => {
  const {treeMarks, isMergedTree, mergedPlan, backendClosePlan} = await import("../model.js");
  for (const fields of [{merged:true}, {mr_state:"merged"}, {state:"finished"}, {body:"/home/dev/demo\n ✓ merged"}]) {
    const tree = {kind:"tree", tree:"demo", ...fields};
    assert.equal(isMergedTree(tree), true);
    assert.equal(treeMarks(tree).merged, true);
    assert.equal(looseReason(tree), "merged · worktree left");
    assert.equal(looseReason({kind:"ticket", children:[tree]}), "merged · worktree left");
  }
  const plan = mergedPlan([
    {kind:"tree", tree:"clean", cwd:"/home/dev/demo/clean", merged:true},
    {kind:"tree", tree:"dirty", merged:true, body:"+1"},
    {kind:"tree", tree:"ahead", merged:true, body:"↑2"},
    {kind:"tree", tree:"open", merged:true, mr_state:"opened"},
    {kind:"tree", tree:"unmerged"},
  ]);
  assert.deepEqual(plan.remove, [{tree:"clean", path:"/home/dev/demo/clean", uncommitted:0, unpushed:0, merged:true}]);
  assert.deepEqual(plan.keep.map(tree => tree.reason), ["uncommitted changes", "unpushed commits", "open MR", "not merged"]);
  assert.deepEqual(mergedPlan().remove, []);
  assert.deepEqual(backendClosePlan({...plan, kind:"task", blockers:["MR !7 is open"],
    close_tickets:["DEMO-1"], leave_tickets:["DEMO-2"], note_done:true, close_space:true}), {
    blockers:["MR !7 is open"], removals:[{name:"clean", path:"/home/dev/demo/clean", changes:0, unpushed:0}],
    keep:plan.keep, jira:["DEMO-1"], leave:["DEMO-2"], also:["Marks the note done", "Closes the Herdr space"], proof:{count:0, latest:null},
  });
  for (const invalid of [null, {}, {...plan, keep:null}]) assert.throws(() => backendClosePlan(invalid), /Invalid close plan/);
});

test("frontmatter parses only leading terminated metadata and skips nested maps", async () => {
  const {parseFrontmatter} = await import("../model.js");
  assert.deepEqual(parseFrontmatter('---\r\nstate: active\r\ntags: [demo, "two words"]\r\ntickets:\r\n  - DEMO-12\r\ncreated: "2026-01-01"\r\nsessions:\r\n  codex:\r\n    - secret\r\n---\r\n# Body'), {
    properties:{state:"active", tags:["demo", "two words"], tickets:["DEMO-12"], created:"2026-01-01", sessions:[]}, body:"# Body",
  });
  for (const text of ["Body\n---\nstate: done\n---", "---\nstate: done", "", null])
    assert.deepEqual(parseFrontmatter(text), {properties:{}, body:text ?? ""});
  assert.deepEqual(parseFrontmatter("---\n---\nBody"), {properties:{}, body:"Body"});
});

test("wikilinks resolve existing local notes; embeds and code remain distinct", async () => {
  const {transformWikilinks, wikiTargets, wikiPath} = await import("../model.js");
  const path = "/home/dev/notes/Task.md", existing = new Set(["/home/dev/notes/Page.md", "/home/dev/notes/sub/Other.md"]);
  assert.equal(transformWikilinks("[[Page]] [[Page|Alias]] [[Missing]] ![[drawing]] [[sub/Other#Heading|Other]]", path, existing),
    "[Page](berbr-note:%2Fhome%2Fdev%2Fnotes%2FPage.md) [Alias](berbr-note:%2Fhome%2Fdev%2Fnotes%2FPage.md) Missing ![[drawing]] [Other](berbr-note:%2Fhome%2Fdev%2Fnotes%2Fsub%2FOther.md)");
  const code = "`[[Code]]`\n```md\n[[Code]]\n![[literal]]\n```\n[[Page]]";
  assert.deepEqual(wikiTargets(code), ["Page"]);
  assert.ok(transformWikilinks(code, path, existing).startsWith("`[[Code]]`\n```md\n[[Code]]\n![[literal]]\n```"));
  for (const target of ["", "#Heading", "../Outside", "/Outside", "https://example.com", "sub/../Other"])
    assert.equal(wikiPath(path, target), null);
  assert.equal(wikiPath(path, "Page.md"), "/home/dev/notes/Page.md");
});

test("panel memoization survives focus redraws and invalidates live, snapshot and filter changes", async () => {
  const {PanelModel} = await import("../model.js");
  const data = {rows:[{id:"task", kind:"task", space:"demo", children:[{kind:"agent", terminal:"pane"}]}]};
  const live = [{address:"pane", status:"working", seq:1}], spaces = [{id:"demo", active:false}];
  const options = {collapsed:new Set()};
  const panel = new PanelModel().update(data, live, spaces, options), task = data.rows[0];
  const values = [panel.sections, panel.visible, panel.signal(task), panel.detail(task), panel.plan(task), panel.related];
  panel.update(data, [...live], [...spaces], options);
  assert.deepEqual([panel.sections, panel.visible, panel.signal(task), panel.detail(task), panel.plan(task), panel.related], values);
  for (const [i, value] of [panel.sections, panel.visible, panel.signal(task), panel.detail(task), panel.plan(task), panel.related].entries())
    assert.equal(value, values[i]);
  panel.update(data, live, spaces, {...options, query:"missing"});
  assert.deepEqual(panel.rows, []);
  assert.equal(panel.detail(task), values[3]);
  live[0].status = "blocked"; live[0].seq++;
  panel.update(data, live, spaces, options);
  assert.equal(panel.signal(task).status, "blocked");
  assert.equal(panel.related, values[5]);
  spaces[0].active = true;
  panel.update(data, live, spaces, options);
  assert.equal(panel.signal(task).liveSpace.active, true);
  assert.notEqual(panel.sections, values[0]);
  panel.update({rows:[]}, live, spaces, options);
  assert.deepEqual(panel.rows, []);
  assert.deepEqual(panel.mergedTrees, []);
});

test("all section kinds are collapsible and filtering temporarily expands them without losing counts", () => {
  const data = {rows:[{id:"task", kind:"task", title:"Demo", children:[]}]};
  const sections = new PanelModel().update(data).sections;
  assert.deepEqual(sections.map(section => section.id), ["needs", "running", "progress", "todo", "cleanup", "done"]);
  assert.ok(sections.every(section => section.collapsible));
  const collapsed = new Set(sections.map(section => section.id));
  const hidden = new PanelModel().update(data, [], [], {collapsed}).visible;
  assert.equal(hidden[0].id, "progress");
  assert.equal(hidden[0].rows.length, 1);
  assert.equal(hidden[0].expanded, false);
  assert.deepEqual(visibleRows(hidden), []);
  const filtered = new PanelModel().update(data, [], [], {collapsed, query:"demo"}).visible;
  assert.equal(filtered[0].expanded, true);
  assert.equal(visibleRows(filtered)[0].id, "task");
  assert.equal(collapsed.size, 6);
});

test("task excerpts stop before machine sections, including when they are the whole body", () => {
  for (const heading of ["## Agent notes", "## aGeNt NoTeS", "## Sessions", "## Сессии", "### Sessions ###"]) {
    assert.equal(taskDetail({body:`Useful **acceptance** criteria.\n\n${heading}\n- machine history`}).excerpt,
      "Useful acceptance criteria.");
    assert.equal(taskDetail({body:`\n${heading}\n- machine history\n\n## Later\nMore history`}).excerpt, "");
  }
  assert.equal(taskDetail({body:"Useful criteria.\r\n\r\n## SESSIONS\r\nMachine history"}).excerpt, "Useful criteria.");
  assert.equal(taskDetail({body:"Agent notes are discussed in this paragraph."}).excerpt, "Agent notes are discussed in this paragraph.");
  assert.equal(taskDetail({body:"## Sessions overview\nHuman criteria"}).excerpt, "Sessions overview Human criteria");
  assert.ok(taskDetail({body:"```markdown\n## Sessions\n```\nHuman criteria"}).excerpt.includes("Human criteria"));
  assert.ok(taskDetail({body:"~~~markdown\n## Agent notes\n~~~\nHuman criteria"}).excerpt.includes("Human criteria"));
});

test("task search indexes preview text, descendant previews and legacy bodies without machine history", () => {
  for (const fields of [{body:"Human acceptance criteria.", body_truncated:true},
    {body:"Human acceptance criteria.\n\n## Agent notes\nMachine history"}]) {
    const child = {kind:"task", id:"task:demo-2", task:"demo-2", title:"Subtask", body:"Nested requirement"};
    const row = {kind:"task", id:"task:demo-1", task:"demo-1", title:"Demo", tags:["tagged"], children:[child], ...fields};
    const model = new PanelModel(), data = {version:3, rows:[row]};
    for (const query of ["HUMAN ACCEPTANCE", "nested requirement", "tagged", "demo-1"]) {
      model.update(data, [], [], {query});
      assert.ok(model.rows.includes(row), query);
    }
    model.update(data, [], [], {query:"Machine history"});
    assert.equal(model.rows.length, 0);
  }
});

test("task details mark truncated snapshots as previews and accept older complete bodies", () => {
  assert.equal(taskDetail({body:"Human criteria.", body_truncated:true}).excerpt, "Human criteria.…");
  assert.equal(taskDetail({body:"Human criteria.", body_truncated:false}).excerpt, "Human criteria.");
  assert.equal(taskDetail({body:"Human criteria.\n\n## Sessions\nMachine history"}).excerpt, "Human criteria.");
  assert.equal(taskDetail({body:"", body_truncated:true}).excerpt, "");
});

test("task spaces accept title, stable id and legacy names without case sensitivity", () => {
  const row = {task:"demo-4", title:"Проверить Parser"};
  for (const label of [" DEMO-4 ", " проверить PARSER "]) assert.equal(taskSpaceMatches(row, label), true);
  for (const label of ["", null, "other"]) assert.equal(taskSpaceMatches(row, label), false);
  const titleSpace = {id:"title-space", name:"Проверить Parser"};
  assert.equal(taskSpace(row, [titleSpace]), titleSpace);
  assert.equal(taskSpace({task:"Legacy title"}, [{id:"legacy", name:"legacy TITLE"}]).id, "legacy");
  const explicit = {id:"known", name:"Custom label"};
  assert.equal(taskSpace({...row, space:"known"}, [titleSpace, explicit]), explicit);
  assert.equal(taskSignals({...row, children:[]}, [], [titleSpace]).liveSpace, titleSpace);
});
