import test from "node:test";
import assert from "node:assert/strict";
import {mergeRequests, mrListPills, PanelModel} from "../model.js";

const mr = (id, extra = {}) => ({kind:"mr", id, label:`!${id} Change`, state:"opened",
  url:`https://gitlab.example/group/repo/-/merge_requests/${id}`, ...extra});

test("MR list collects all nesting levels, deduplicates URLs and retains each owning task", () => {
  const first = mr(1), second = mr(2), otherRepo = mr(1, {url:"https://gitlab.example/other/repo/-/merge_requests/1"});
  const child = {kind:"task", id:"child", task:"child-id", title:"Child task", children:[first]};
  const task = {kind:"task", id:"parent", task:"parent-id", title:"Parent task", done:true, children:[
    {kind:"ticket", children:[first]}, {kind:"tree", repo:"/home/repo", children:[{...first, id:"duplicate"}]}, child]};
  const rows = [task, {kind:"header", children:[second, otherRepo, {...second, url:second.url + "/"}]}];
  const before = JSON.stringify(rows);
  const list = mergeRequests(rows);
  assert.equal(list.length, 3);
  assert.equal(new Set(list.map(row => row.id)).size, 3);
  const entry = list.find(row => row.url === first.url);
  assert.equal(entry.repository, "group/repo");
  assert.deepEqual(entry.tasks.map(task => task.id), ["parent", "child"]);
  assert.match(entry.searchText, /parent-id parent task/);
  assert.match(entry.searchText, /child-id child task/);
  assert.equal(list.find(row => row.url === second.url).tasks.length, 0);
  assert.equal(JSON.stringify(rows), before);
});

test("MR list handles older snapshots, absent URLs and completed state without merging equal IIDs across repos", () => {
  const list = mergeRequests([{kind:"tree", repo:"/home/local", children:[mr(2, {url:null, body:"ci✗ draft", branch:"feature"})]},
    mr(1, {state:"MERGED"}), mr(3, {state:"closed"})]);
  assert.equal(list.find(row => !row.url).repository, "/home/local");
  assert.match(list.find(row => !row.url).searchText, /feature/);
  assert.equal(list.filter(row => row.completed).length, 2);
  assert.deepEqual(mergeRequests(), []);
});

test("MR list exposes simultaneous CI/review flags and distinguishes unknown or unusual CI states", () => {
  const labels = row => mrListPills(row).map(spec => spec.label);
  assert.deepEqual(labels(mr(1, {ci:"failed", draft:true, conflict:true, approvals:2, unresolved:3})),
    ["CI failed", "draft", "conflict", "2 approvals", "3 threads"]);
  assert.deepEqual(labels(mr(1, {ci:"success", ready:true, approvals:1})), ["CI passed", "1 approval", "ready to merge"]);
  assert.deepEqual(labels(mr(1)), ["CI unknown"]);
  assert.deepEqual(labels(mr(1, {ci:"canceled"})), ["CI canceled"]);
  assert.deepEqual(labels(mr(1, {body:"ci✓ draft"})), ["CI passed", "draft"]);
  assert.deepEqual(labels(mr(1, {state:"merged", ci:"failed", conflict:true})), ["merged"]);
});

test("MR index is built lazily, reused during live updates and replaced with a new snapshot", () => {
  const data = {rows:[mr(1)]};
  const model = new PanelModel().update(data);
  assert.equal(model.mrEntries, null);
  const entries = model.mrs;
  model.update(data, [{address:"agent", status:"working"}]);
  assert.equal(model.mrs, entries);
  model.update({rows:[mr(2)]});
  assert.notEqual(model.mrs, entries);
  assert.equal(model.mrs[0].label, "!2 Change");
});

test("complete backend catalog includes MRs omitted from the tree and keeps full title and task associations", () => {
  const task = {id:"task:demo", task:"demo", title:"Parser work"};
  const first = mr(1, {label:"!1 DEMO-123 Fix parser", title:"DEMO-123 Fix parser", branch:"fix-parser",
    repository:"group/repo", tickets:["DEMO-123"], tasks:[task]});
  const second = mr(2, {branch:"fix-parser"});
  const data = {rows:[{kind:"task", ...task, children:[{...second, label:"!2 Shortened"}]}],
    merge_requests:[first, second, mr(3, {state:"closed"})]};
  const list = new PanelModel().update(data).mrs;
  assert.equal(list.length, 3);
  const entry = list.find(row => row.url === first.url);
  assert.match(entry.searchText, /demo-123/);
  assert.match(entry.searchText, /parser work/);
  assert.equal(entry.label, first.label);
  assert.equal(list.find(row => row.url === second.url).tasks.length, 1);
  assert.equal(list.filter(row => !row.completed).length, 2);
});

test("older tree-only snapshots retain stripped Jira keys from every parent ticket", () => {
  const row = mr(7, {label:"!7 Fix parser", branch:"fix-parser"});
  const list = mergeRequests([{kind:"ticket", target:"DEMO-123", children:[row]},
    {kind:"ticket", target:"DEMO-456", children:[row]}]);
  assert.equal(list.length, 1);
  assert.match(list[0].searchText, /demo-123 demo-456/);
});
