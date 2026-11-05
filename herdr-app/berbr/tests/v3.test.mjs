import test from "node:test";
import assert from "node:assert/strict";
import {readFile} from "node:fs/promises";
import {blockedPrompt, mrFlags, taskSignals, groupSections, jumpItems, matchJump} from "../model.js";

const fixture = name => readFile(new URL(`fixtures/${name}`, import.meta.url), "utf8");

test("numbered permission and trust fixtures expose choices, supplied unnumbered captures do not", async () => {
  const status = {kind:"permission", msg:"approve Fetch: https://example.com", state:"blocked"};
  const permission = blockedPrompt(status, await fixture("numbered-permission.txt"));
  assert.equal(permission.question, "Fetch: https://example.com");
  assert.deepEqual(permission.choices.map(choice => choice.key), ["1", "2", "3"]);
  assert.equal(permission.choices[1].label, "Yes, and don't ask again for this site");
  assert.equal(blockedPrompt(status, await fixture("numbered-trust.txt")).choices.length, 2);
  for (const name of ["claude-permission-webfetch.txt", "claude-trust-folder.txt", "claude-ask-user-question.txt"])
    assert.deepEqual(blockedPrompt(status, await fixture(name)).choices, []);
  for (const name of ["claude-permission-webfetch.get.json", "claude-ask-user-question.get.json"]) {
    const info = JSON.parse(await fixture(name));
    assert.equal(info.cwd, "/home/dev/demo");
    assert.equal(info.foreground.cwd, "/home/dev/demo");
    assert.equal(info.status.state, "working");
  }
});

test("prompt parsing uses the last bottom numbered block and strips answer prefixes", () => {
  const status = {kind:"question", msg:"answer: Which colour? (Red · Green · Blue)"};
  const screen = "1. Old\n2. Transcript\n\nWhich colour?\n\u001b[32m❯ 1. Red\u001b[0m\n  2. Green\n  3. Blue\nEnter to select";
  const prompt = blockedPrompt(status, screen);
  assert.equal(prompt.question, "Which colour? (Red · Green · Blue)");
  assert.deepEqual(prompt.choices.map(choice => choice.label), ["Red", "Green", "Blue"]);
  assert.deepEqual(blockedPrompt(status, "1. Old\n2. Transcript\nWorking on something else").choices, []);
  for (const screen of [null, "", "❯ Yes\n  No", "2. No first option"])
    assert.deepEqual(blockedPrompt(status, screen).choices, []);
  assert.equal(blockedPrompt(null, null).question, "");
  assert.equal(blockedPrompt({msg:"answer: "}, "").question, "");
});

const mr = overrides => ({id:"mr", kind:"mr", state:"opened", label:"!123 Demo change", body:"ci✗ conflict draft",
  ci:"success", approvals:2, unresolved:0, draft:false, conflict:false, merge_status:"mergeable", ready:false, ...overrides});

test("v3 MR fields decide whose move; absent fields keep v2 section behavior", () => {
  for (const [fields, reason] of [[{ci:"failed"}, "CI failed"], [{conflict:true}, "conflict"],
    [{unresolved:2}, "2 threads"], [{unresolved:1}, "1 thread"]]) {
    const row = mr(fields), task = {id:"task", kind:"task", children:[row]};
    assert.equal(mrFlags(row).worst, reason);
    assert.equal(taskSignals(task).mrFlag, reason);
    assert.equal(groupSections([task]).find(section => section.id === "needs").rows[0], task);
  }
  assert.equal(mrFlags(mr({ready:true})).worst, "ready to merge");
  assert.equal(mrFlags(mr({draft:true})).worst, "draft");
  assert.equal(mrFlags(mr({ci:"pending"})).ci, "running");
  assert.equal(mrFlags(mr({ci:null})).ci, null);
  const legacy = {kind:"mr", state:"opened", body:"ci✗ conflict draft"};
  assert.equal(mrFlags(legacy).worst, "CI failed");
  assert.equal(mrFlags(legacy).needsYou, false);
  const task = {id:"v2", kind:"task", children:[legacy]};
  assert.equal(groupSections([task]).find(section => section.id === "progress").rows[0], task);
});

test("an actionable MR takes precedence across multiple MRs and a working agent", () => {
  const task = {kind:"task", children:[mr({id:"ready", ready:true}), mr({id:"failed", ci:"failed"}), {kind:"agent", terminal:"demo"}]};
  const signal = taskSignals(task, [{address:"demo", status:"working"}]);
  assert.equal(signal.status, "blocked");
  assert.equal(signal.mrs[0].id, "failed");
  assert.equal(signal.mrNeedsYou, true);
  task.done = true;
  assert.equal(taskSignals(task).status, "closed");
});

test("jump index covers snapshot kinds, deduplicates MRs and matches fuzzy title/key/iid/branch", () => {
  const change = mr({url:"https://example.com/mr/123"});
  const task = {kind:"task", id:"task", title:"Demo refactor", children:[
    {kind:"ticket", id:"ticket", target:"DEMO-12", children:[change]},
    {kind:"tree", id:"tree", branch:"feature/button-colour", children:[change, {kind:"agent", id:"agent", terminal:"demo"}]},
  ]};
  const items = jumpItems({project:"demo", rows:[task], projects:[{name:"demo"}, {name:"other"}]}, [{address:"demo", title:"Waiting for colour"}]);
  assert.deepEqual(items.map(item => item.kind).sort(), ["agent", "mr", "project", "task", "ticket", "tree"]);
  for (const [query, kind] of [["dm rfct", "task"], ["DEMO-12", "ticket"], ["123", "mr"], ["btnclr", "tree"], ["Waiting colour", "agent"], ["other", "project"]])
    assert.ok(matchJump(items, query).some(item => item.kind === kind), query);
  assert.deepEqual(matchJump(items, "zzzz"), []);
  assert.deepEqual(jumpItems(null), []);
});

test("Codex approval choices accept the selected marker › as well as ❯ and >", () => {
  const status = {kind:"permission", msg:"approve command", state:"blocked"};
  for (const marker of ["›", "❯", ">"])
    assert.deepEqual(blockedPrompt(status, `${marker} 1. Yes\n  2. No\nEnter to confirm`).choices,
      [{key:"1", label:"Yes"}, {key:"2", label:"No"}]);
});

test("question choices retain descriptions and intervening blanks without including them in labels", async () => {
  const screen = await fixture("numbered-question.txt");
  const status = {kind:"question", msg:"answer: Which colour?", state:"blocked"};
  const expected = [{key:"1", label:"Red"}, {key:"2", label:"Green"}, {key:"3", label:"Blue"}, {key:"4", label:"Type something."}];
  assert.deepEqual(blockedPrompt(status, screen).choices, expected);
  assert.deepEqual(blockedPrompt(status, screen + "Working on something else\n").choices, []);
  assert.deepEqual(blockedPrompt(status, screen + "Working on something else\n      Indented output\n").choices, []);
  assert.deepEqual(blockedPrompt(status, "1. Old choice\n2. Old choice\nNew question?\n" + screen).choices, expected);
  assert.deepEqual(blockedPrompt(status, "1. Yes\n     Description\n2. No\n     Another description").choices,
    [{key:"1", label:"Yes"}, {key:"2", label:"No"}]);
});
