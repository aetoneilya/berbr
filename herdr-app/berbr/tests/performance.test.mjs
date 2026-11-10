import test from "node:test";
import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import { PanelModel } from "../model.js";

// Wall-clock budgets are native-runner baselines, not architecture-independent
// algorithmic limits. Node 26 on GitHub's Intel runner measured 41 ms; the M1
// runner meets 15 ms. Keep a regression margin for Intel without weakening M1.
const coldBudgetMs = process.arch === "x64" ? 50 : 15;
test(`400-task cold model median stays under ${coldBudgetMs} ms on ${process.arch}`, t => {
  const rows = Array.from({length:400}, (_, index) => {
    const task = `Demo ${index}`;
    const mrs = Array.from({length:6}, (_, number) => ({kind:"mr", id:`mr-${index}-${number}`,
      label:`!${index * 6 + number} Demo change`, state:number % 2 ? "merged" : "opened", body:"ci✓"}));
    return {kind:"task", id:`task-${index}`, task, title:task, body:"Demo **acceptance** criteria", children:[
      ...Array.from({length:3}, (_, number) => ({kind:"ticket", id:`ticket-${index}-${number}`,
        target:`DEMO-${index * 3 + number}`, state:"In progress", children:[mrs[number]]})),
      ...Array.from({length:4}, (_, number) => ({kind:"tree", id:`tree-${index}-${number}`,
        tree:`checkout-${index}-${number}`, cwd:`/home/dev/demo/${index}/${number}`, merged:number === 1,
        children:[mrs[number], {kind:"agent", terminal:`pane-${index}-${number}`}]})),
      mrs[4], mrs[5],
    ]};
  });
  const data = {version:2, rows};
  const agents = rows.map((_, index) => ({address:`pane-${index}-0`, status:index % 2 ? "working" : "blocked", seq:index}));
  const times = [];
  for (let run = 0; run < 20; run++) {
    const start = performance.now();
    const panel = new PanelModel().update(data, agents, [], {query:"demo", collapsed:new Set(), expanded:rows[0].id});
    for (const row of panel.rows) {
      panel.signal(row);
      panel.detail(row);
      panel.plan(row);
    }
    assert.equal(panel.rows.length, 400);
    times.push(performance.now() - start);
  }
  times.sort((a, b) => a - b);
  const median = (times[9] + times[10]) / 2;
  t.diagnostic(`Median of 20 cold full model paths: ${median.toFixed(2)} ms`);
  assert.ok(median < coldBudgetMs, `Median ${median.toFixed(2)} ms exceeds the ${coldBudgetMs} ms ${process.arch} render-model budget`);
});

test("300-task idle timer tick shares one live model and stays under a generous 25 ms median", async t => {
  const {registerHooks} = await import("node:module");
  const runtime = new URL("./support/runtime.mjs", import.meta.url);
  registerHooks({resolve(specifier, context, next) {
    return ["gpui-kit", "gpui-base", "herdr", "fs/promises"].includes(specifier)
      ? {url:runtime.href, shortCircuit:true} : next(specifier, context);
  }});
  const {state} = await import(runtime);
  const {default:Berbr} = await import("../main.js");
  const agents = [], spaces = [];
  const rows = Array.from({length:300}, (_, index) => {
    spaces.push({id:`space-${index}`, name:`Demo ${index}`, tabs:[]});
    return {kind:"task", id:`task-${index}`, title:`Demo ${index}`, children:
      Array.from({length:5}, (_, tree) => ({kind:"tree", id:`tree-${index}-${tree}`, children:
        Array.from({length:3}, (_, number) => {
          const address = `agent-${index}-${tree}-${number}`;
          agents.push({address, pane:address, space:`space-${index}`, status:"working", seq:1});
          return {kind:"agent", terminal:address};
        })}))};
  });
  Object.assign(state, {props:{}, agents, spaces, focused:null, shared:new Map()});
  globalThis.localStorage = {getItem:() => null, setItem() {}};
  const timers = [], cx = {notify() {}, focus_handle:() => ({focus() {}, is_focused:() => true}),
    timer:{every:(ms, fn) => timers.push({ms, fn})}, spawn() {}};
  const view = new Berbr(); view.init({}, cx); view.loading = false; view.data = {rows};
  const tick = timers.find(timer => timer.ms === 1000).fn;
  tick(cx);
  const update = view.panelModel.update.bind(view.panelModel);
  let updates = 0;
  view.panelModel.update = (...args) => {updates++; return update(...args);};
  const times = [];
  for (let index = 0; index < 25; index++) {
    updates = 0;
    const started = performance.now(); tick(cx); times.push(performance.now() - started);
    assert.equal(updates, 1, "one update for all 300 tasks / 4500 agents");
  }
  times.sort((a, b) => a - b);
  t.diagnostic(`300-task idle tick median: ${times[12].toFixed(2)} ms`);
  assert.ok(times[12] < 25, `Idle tick median ${times[12].toFixed(2)} ms exceeds 25 ms`);
  for (const interval of [30000, 300000]) {
    view.loading = false; updates = 0;
    timers.find(timer => timer.ms === interval).fn(cx);
    assert.equal(updates, 1, `one model update per ${interval} ms timer pass`);
  }
  view.loading = false; updates = 0;
  view.key({key:"down"}, {...cx, stop_propagation() {}});
  assert.equal(updates, 1, "one model update for keyboard navigation");
});
