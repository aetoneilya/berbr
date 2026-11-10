import test from "node:test";
import assert from "node:assert/strict";
import {smartTarget, newTaskArgs, forgeName, changeKind, changeNumber, closePlan, rowMenu,
  mrFlags, mrPills, freeTickets, groupSections, taskSignals, taskDetail, mergedPlan} from "../model.js";

const issue = {id:"issue",kind:"ticket",provider:"github",target:"github.com/demo/repo#1",label:"github.com/demo/repo#1  Fix login",state:"open",url:"https://github.com/demo/repo/issues/1"};
const pr = {id:"pr",kind:"mr",provider:"github",label:"#7  Fix login",state:"open",url:"https://github.com/demo/repo/pull/7",branch:"same"};

test("GitHub issue URLs and qualified references produce canonical CLI arguments", () => {
  for (const text of ["Demo/Repo#1", "github.com/Demo/Repo#1", "https://github.com/Demo/Repo/issues/1"]) {
    assert.deepEqual(smartTarget(text),{kind:"ticket",key:issue.target});
    const result = newTaskArgs({source:"github",text,agent:"codex",worktree:true});
    assert.deepEqual(result.args,["start",issue.target,"--bg","--agent","codex"]);
  }
  assert.deepEqual(newTaskArgs({source:"github",text:"#1",agent:"none",worktree:false}).args,
    ["start","#1","--bg","--no-agent","--no-worktree"]);
  for (const text of ["DEMO-1", "https://github.com/demo/repo/pull/7", "https://enterprise.invalid/demo/repo/issues/1", "demo/repo#0"])
    assert.match(newTaskArgs({source:"github",text,worktree:true}).error,/GitHub issue/);
  assert.match(newTaskArgs({source:"jira",text:issue.target,worktree:true}).error,/Jira ticket/);
});

test("GitHub menus, PR labels and review state preserve provider terminology", () => {
  assert.equal(forgeName(pr),"GitHub"); assert.equal(changeKind(pr),"PR"); assert.equal(changeNumber(pr),"#7");
  assert.equal(rowMenu(pr)[0].label,"Open in GitHub");
  assert.equal(rowMenu(issue)[0].label,"Open in GitHub");
  assert.equal(rowMenu({kind:"tree",cwd:"/synthetic/tree"},{mr:pr}).find(item => item.id === "mr").label,"Open PR");
  const fields = {...pr,ci:"success",ready:false,review_decision:"CHANGES_REQUESTED"};
  assert.equal(mrFlags(fields).needsYou,true);
  assert.equal(mrPills(fields)[0].label,"changes requested");
  assert.ok(!mrPills({...fields,ci:null,review_decision:null}).some(p => p.label === "ready to merge"));
  assert.deepEqual(mrPills({...fields,state:"merged",ready:true}).map(p => p.label),["merged"]);
});

test("GitHub issues populate Todo and close previews leave them open", () => {
  const snapshot = {rows:[{id:"issues",kind:"header",label:"GitHub issues"},issue]};
  assert.deepEqual(freeTickets(snapshot),[{key:issue.target,title:"Fix login"}]);
  assert.ok(groupSections(snapshot.rows).find(section => section.id === "todo").rows.includes(issue));
  const plan = closePlan({kind:"task",children:[issue,pr]});
  assert.deepEqual(plan.jira,[]);
  assert.deepEqual(plan.blockers,["PR #7 is open"]);
});


test("open GitHub PRs drive task summaries, details and removal blockers", () => {
  const row = {id:"task",kind:"task",task:"demo-0",children:[{...pr,ci:"failed",ready:false}]};
  assert.equal(taskSignals(row).status,"blocked");
  assert.equal(taskSignals(row).mrKey,"#7");
  assert.equal(taskDetail(row).openMrs.length,1);
  const tree = {id:"tree",kind:"tree",tree:"synthetic",children:[pr]};
  assert.equal(mergedPlan([tree]).keep[0].reason,"open PR");
});
