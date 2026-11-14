import test from "node:test";
import assert from "node:assert/strict";
import {mrPills, mrFlags, focusedTask, focusedTree} from "../model.js";

test("MR pills retain worst-flag priority, CI glyphs and theme tones", () => {
  for (const [fields, expected] of [
    [{ci:"failed", conflict:true}, {label:"CI failed", tone:"blocked", glyph:"✕", spin:false}],
    [{ci:"running"}, {label:"CI running", tone:"working", glyph:"spinner", spin:true}],
    [{ci:"success"}, {label:"CI passed", tone:"done", glyph:"check", spin:false}],
    [{conflict:true}, {label:"conflict", tone:"blocked", glyph:null, spin:false}],
    [{unresolved:2}, {label:"2 threads", tone:"blocked", glyph:null, spin:false}],
    [{unresolved:1}, {label:"1 thread", tone:"blocked", glyph:null, spin:false}],
    [{draft:true}, {label:"draft", tone:"dim", glyph:null, spin:false}],
    [{ready:true}, {label:"ready to merge", tone:"accent", glyph:"check", spin:false}],
  ]) assert.deepEqual(mrPills({state:"opened", ...fields})[0], expected);
  assert.deepEqual(mrPills({}), []);
  assert.equal(mrPills({body:"ci✗"})[0].tone, "blocked");
  assert.equal(mrFlags({body:"ci✗"}).needsYou, false);
  assert.deepEqual(mrPills({unresolved:2, ci:"running"}).map(pill => pill.label), ["2 threads", "CI running"]);
  assert.deepEqual(mrPills({draft:true, ci:"success"}).map(pill => pill.label), ["draft", "CI passed"]);
  assert.deepEqual(mrPills({ready:true, ci:"success"}).map(pill => pill.label), ["ready to merge"]);
});

test("historical MR pills suppress stale flags for merged and closed MRs", () => {
  for (const state of ["merged", "closed"]) {
    assert.deepEqual(mrPills({state, ci:"failed", unresolved:2, ready:true}),
      [{label:state, tone:"dim", glyph:null, spin:false}]);
  }
  assert.equal(mrPills({ci:"failed"}, {historical:true})[0].tone, "dim");
  assert.equal(mrPills({ci:"failed"}, {state:"merged"})[0].label, "merged");
});

test("focused task prefers a terminal's owner, including address aliases, over the space", () => {
  const first = {id:"a", kind:"task", task:"alpha", children:[{kind:"agent", terminal:"pane-a"}]};
  const second = {id:"b", kind:"task", title:"Beta"};
  const rows = [{kind:"header", children:[first, second]}];
  const spaces = [{id:"space-b", name:"Beta"}];
  assert.equal(focusedTask(rows, {space:"space-b", terminal:"pane-a"}, spaces), first);
  assert.equal(focusedTask(rows, {space:"space-b", terminal:"address-a"}, spaces,
    [{address:"address-a", pane:"pane-a"}]), first);
  assert.equal(focusedTask(rows, {space:"space-b", terminal:"unknown"}, spaces), second);
  assert.equal(focusedTask(rows, {space:"unknown"}, spaces), null);
  assert.equal(focusedTask(rows, null, spaces), null);
});

test("focused worktree uses the longest cwd ancestor with path boundaries", () => {
  const parent = {kind:"tree", cwd:"/home/demo/project/"};
  const nested = {kind:"tree", body:"/home/demo/project/nested\n +2"};
  const other = {kind:"tree", cwd:"/home/demo/project-next"};
  const rows = [{kind:"task", children:[parent, nested, other, {kind:"tree"}]}];
  assert.equal(focusedTree(rows, "/home/demo/project/nested/src"), nested);
  assert.equal(focusedTree(rows, "/home/demo/project/"), parent);
  assert.equal(focusedTree(rows, "/home/demo/project-next/src"), other);
  assert.equal(focusedTree(rows, "/home/demo/project-other"), null);
  assert.equal(focusedTree(rows, null), null);
});

test("CI separates proven review gates, queued work, manual action, and cached evidence", () => {
  const gate = {context:"codereview", status:"running", pipeline_id:7, source_sha:"source", pipeline_sha:"merged"};
  const labels = fields => mrPills({state:"opened", ...fields}).map(p => p.label);
  assert.deepEqual(labels({ci:"running", ci_review:gate, ci_freshness:"Live"}), ["CI review"]);
  assert.equal(mrPills({ci:"running", ci_review:gate})[0].spin, false);
  for (const ci_freshness of ["Live", "Cached"]) {
    const review = mrPills({ci:"running", ci_review:gate, ci_freshness})[0];
    assert.equal(review.tone, "working");
    assert.equal(review.glyph, "clock");
    assert.equal(review.spin, false);
  }
  for (const [ci, label] of [["pending", "CI pending"], ["created", "CI queued"],
    ["preparing", "CI preparing"], ["waiting_for_resource", "CI waiting for resource"],
    ["manual", "CI manual action"], ["blocked", "CI manual action"]]) {
    const pills = mrPills({ci, ci_freshness:"Live"});
    assert.equal(pills[0].label, label);
    assert.equal(pills[0].spin, false);
  }
  for (const fields of [{ci:"running", merge_status:"not_approved"},
    {ci:"running", ci_review:{...gate, context:"ai-review"}},
    {ci:"running", ci_review:{...gate, status:"pending"}}]) {
    assert.deepEqual(labels(fields), ["CI running"]);
    assert.equal(mrPills(fields)[0].spin, true);
  }
  assert.deepEqual(labels({ci:"running", ci_review:gate, ci_freshness:"Cached"}), ["CI review"]);
  const cached = mrPills({ci:"running", ci_freshness:"Cached"})[0];
  assert.equal(cached.label, "CI running");
  assert.equal(cached.spin, false);
  assert.deepEqual(labels({ci:"running", ci_review:gate, ci_freshness:"Missing"}), ["CI unknown"]);
  assert.deepEqual(labels({body:"ci:review"}), ["CI review"]);
  assert.deepEqual(labels({body:"ci:manual"}), ["CI manual action"]);
  assert.deepEqual(labels({body:"ci:unknown"}), ["CI unknown"]);
  assert.deepEqual(labels({body:"ci…"}), ["CI running"]);
});

test("terminal and unfamiliar CI states keep clean focused labels", () => {
  for (const ci of ["canceled", "skipped", "future-server-status"]) {
    for (const ci_freshness of [undefined, "Live", "Cached"]) {
      const [pill] = mrPills({ci, ci_freshness});
      assert.equal(pill.label, `CI ${ci}`);
      assert.equal(pill.tone, "dim");
      assert.equal(pill.spin, false);
    }
  }
});
