import test from "node:test";
import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";
import { PanelModel } from "../model.js";

test("400-task panel model full render path has median under 15 ms over 20 cold runs", t => {
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
  assert.ok(median < 15, `Median ${median.toFixed(2)} ms exceeds the 15 ms render-model budget`);
});
