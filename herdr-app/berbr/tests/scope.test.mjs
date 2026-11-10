import test from "node:test";
import assert from "node:assert/strict";
import {combineProjects, protectSpaceBindings, scopeSummary, ProjectLoader} from "../scope.js";
import {PanelModel, taskSpace, rowMenu, closePlan} from "../model.js";

test("combined projects namespace UI identity while keeping all command targets and MR associations intact", () => {
  const snapshot = project => ({project, store:project === "one" ? "vault" : "berbr", projects:[{name:project}],
    warnings:["cached GitLab"], rows:[{id:"task:same", kind:"task", task:"same", title:"Same name", children:[
      {id:"tree:checkout", kind:"tree", tree:"checkout", cwd:`/${project}/checkout`, children:[
        {id:"agent:terminal", kind:"agent", terminal:`terminal-${project}`, session:"conversation"}]}]}],
    merge_requests:[{id:"mr:1", kind:"mr", label:"!1 Change", url:`https://gitlab/${project}/-/merge_requests/1`,
      tasks:[{id:"task:same", task:"same", title:"Same name"}]}]});
  const one = snapshot("one"), two = snapshot("two");
  const original = JSON.stringify([one, two]);
  const data = combineProjects([one, two], one, new Map([["two", "offline"]]));
  assert.equal(new Set(data.rows.map(row => row.id)).size, 2);
  assert.deepEqual(data.rows.map(row => row.task), ["same", "same"]);
  assert.deepEqual(data.rows.map(row => row.project), ["one", "two"]);
  assert.equal(data.rows[1].children[0].cwd, "/two/checkout");
  assert.equal(data.rows[1].children[0].children[0].terminal, "terminal-two");
  assert.equal(data.merge_requests[1].tasks[0].id, data.rows[1].id);
  assert.deepEqual(data.projects.map(project => project.name), ["one", "two"]);
  assert.equal(data.project_stores.two, "berbr");
  assert.deepEqual(data.warnings, ["one: cached GitLab", "two: cached GitLab", "two: offline"]);
  assert.equal(JSON.stringify([one, two]), original);
  const model = new PanelModel().update(data, [], [], {query:"two"});
  assert.equal(model.rows.length, 1);
  assert.equal(model.rows[0].project, "two");
  assert.equal(model.mrs.length, 2);
});

test("empty scopes retain project choices and MR fallback supports older backends", () => {
  const base = {project:"one", projects:[{name:"one"}], rows:[{kind:"mr", id:"old-mr", label:"!1 Change", state:"opened"}]};
  assert.deepEqual(combineProjects([], base).rows, []);
  assert.deepEqual(combineProjects([], base).projects, base.projects);
  assert.equal(new PanelModel().update(combineProjects([base], base)).mrs.length, 1);
  assert.equal(scopeSummary([], base.projects, "one"), "No projects");
  assert.equal(scopeSummary(null, base.projects, "one"), "one");
  assert.equal(scopeSummary(["one"], base.projects, "one"), "one");
  assert.equal(scopeSummary(["one", "two"], [{name:"one"},{name:"two"}], "one"), "All projects");
});

test("scoped tasks never borrow a same-name space and conflicting backend space bindings are rejected", () => {
  const spaces=[{id:"one-space", name:"Shared"}];
  assert.equal(taskSpace({project:"two", task:"same", title:"Shared"}, spaces), undefined);
  assert.equal(taskSpace({project:"two", space:"gone", title:"Shared"}, spaces), undefined);
  assert.equal(taskSpace({project:"one", space:"one-space", title:"Shared"}, spaces), spaces[0]);
  const data=combineProjects(["one","two"].map(project=>({project, rows:[{kind:"task", id:"same", task:"same", title:"Shared",
    note:`/${project}/note.md`, space:"one-space"}]})));
  assert.ok(data.rows.every(row=>!taskSpace(row, spaces)));
  assert.ok(data.rows.every(row=>!rowMenu(row,{spaces}).some(action=>["start","done","rename"].includes(action.id))));
  assert.ok(closePlan(data.rows[1],spaces).blockers.some(text=>text.includes("shared by tasks")));
  assert.match(data.warnings.join(" "), /Herdr space shared/);
});

test("a shared MR merges task and ticket associations from all catalogs, even when absent from each tree", () => {
  const data=combineProjects(["one","two"].map(project=>({project, rows:[], merge_requests:[{kind:"mr", id:"mr", label:"!1 Shared",
    url:"https://gitlab/shared/-/merge_requests/1", tasks:[{id:"same", task:project, title:`${project} task`}], tickets:[`${project}-1`]}]})));
  const entries=new PanelModel().update(data).mrs;
  assert.equal(entries.length, 1);
  assert.deepEqual(entries[0].tasks.map(task=>task.project), ["one","two"]);
  assert.match(entries[0].searchText, /one-1 two-1/);
  assert.match(entries[0].searchText, /two task/);
});

test("the shared loader bounds backend work across scope changes and never reuses a previous generation", async () => {
  const pending=[],started=[]; let active=0,maximum=0,epoch=0;
  const loader=new ProjectLoader((project,cached)=>{
    started.push([project,cached]);active++;maximum=Math.max(maximum,active);
    return new Promise(resolve=>pending.push({project,finish:value=>{active--;resolve(value);}}));
  },(_project,generation)=>generation===epoch);
  const old=loader.request("one",false,epoch), second=loader.request("two",false,epoch);
  epoch=1;
  const current=loader.request("one",false,epoch), fourth=loader.request("four",true,epoch);
  assert.equal(started.length,2);
  pending[0].finish("old"); await old;
  assert.equal(started.length,3);
  pending[1].finish("two"); await second;
  assert.equal(started.length,4);
  pending[2].finish("new");pending[3].finish("four");
  assert.equal(await current,"new");await fourth;
  assert.equal(maximum,2);
});


test("space conflicts survive hiding a project and restart, until explicit bindings separate", () => {
  const snapshot = project => ({project, rows:[{kind:"task", id:"task:same", task:"same", note:`/${project}/note.md`,
    space:"shared", children:[{kind:"tree", id:"tree:same", tree:"same"}]}]});
  const one=snapshot("one"), two=snapshot("two"), known=[one,two], conflicts=new Map();
  assert.ok(combineProjects(known, {}, new Map(), known, conflicts).rows.every(row=>row.scope_space_ambiguous));
  const hidden=combineProjects([two], {}, new Map(), known, conflicts);
  assert.equal(hidden.rows[0].scope_space_ambiguous, true);
  assert.equal(hidden.rows[0].children[0].scope_space_ambiguous, true);
  const restored=new Map(JSON.parse(JSON.stringify([...conflicts])));
  const ordinary=protectSpaceBindings(two, [two], restored);
  assert.equal(ordinary.rows[0].id, "task:same");
  assert.equal(ordinary.rows[0].scope_space_ambiguous, true);
  assert.equal(taskSpace(ordinary.rows[0], [{id:"shared", name:"Shared"}]), undefined);
  assert.ok(!rowMenu(ordinary.rows[0], {spaces:[{id:"shared", name:"Shared"}]}).some(item=>item.id === "space"));
  assert.ok(closePlan(ordinary.rows[0], [{id:"shared"}]).blockers.length);
  assert.equal(two.rows[0].scope_space_ambiguous, undefined);
  const rebound={...two, rows:[{...two.rows[0], space:"separate"}]};
  assert.equal(protectSpaceBindings(rebound,[rebound],restored).rows[0].scope_space_ambiguous, undefined);
  assert.equal(restored.has(JSON.stringify(["two","/two/note.md"])), false);
  const fullyResolved=combineProjects([one,rebound], {}, new Map(), [one,rebound], conflicts);
  assert.ok(fullyResolved.rows.every(row=>!row.scope_space_ambiguous));
  assert.equal(conflicts.size, 0);
});
