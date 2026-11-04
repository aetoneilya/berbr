import { View, div } from "gpui-kit";
import { h_flex, v_flex, Input, InputState, TextView } from "gpui-base";
import { appHome, exec, focused, spaces, panes, agents, theme, run, props,
  openOverlay, closeOverlay, notify, shared, setShared, nextCommand } from "herdr";

const ICON = { task: "○", tree: "⌂", mr: "↗", ticket: "◇", agent: "●" };
const tones = (t, state) => ({ active:t.working, working:t.working, review:t.blocked,
  blocked:t.blocked, done:t.done, finished:t.done, merged:t.done, idle:t.idle }[state] ?? t.dim);
const walk = (rows) => rows.flatMap(r => [r, ...walk(r.children ?? [])]);

export default class Berbr extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.binary = p.binary ?? `${appHome()}/mods/berbr/berbr`;
    this.form = p.form ?? null;
    this.project = p.project ?? shared("berbr.project") ?? null;
    this.data = null; this.error = null; this.busy = false; this.loading = false;
    this.filter = ""; this.all = false; this.selected = null; this.folded = new Set();
    this.opening = focused();
    this.cwd = this.opening?.cwd ?? null;
    this.handle = cx.focus_handle();
    this.handle.focus();
    this.inputs = {};
    this.values = {};
    for (const key of ["search", "title", "ticket", "branch", "name"]) {
      const input = InputState.new({ value: "", placeholder: {search:"Search tasks, tickets, branches…",title:"Task title",ticket:"Optional · DEMO-123",branch:"Automatic",name:"Automatic"}[key] });
      input.on("change", (_event, cx) => {
        this.values[key] = input.value();
        if (key === "search") this.filter = input.value();
        cx.notify();
      });
      this.inputs[key] = input;
    }
    this.agent = p.agent ?? "codex"; this.projectMenu = false; this.worktree = true; this.newTicket = false;
    if (!this.form) {
      this.refresh(cx, true);
      cx.timer.every(30000, (cx) => this.refresh(cx, true));
      cx.timer.every(300000, (cx) => this.refresh(cx));
      cx.spawn(async (cx) => { for (;;) { const command = await nextCommand();
        if (command === "refresh") this.refresh(cx, true);
        if (command === "new") this.formOverlay("new");
        if (command === "details" && this.chosen()) this.formOverlay("detail", {row:this.chosen()});
        if (command === "focus") this.handle.focus();
        cx.notify();
      } });
    } else if (this.form === "new") {
      this.inputs.title.focus();
    }
  }

  async cli(args) {
    const env = ["-u", "HERDR_PLUGIN_CONTEXT_JSON", "-u", "HERDR_PLUGIN_ROOT", "-u", "HERDR_ENV", "BERBR_BACKEND=herdr-app", "NO_COLOR=1"];
    // Explicit opening context; it belongs to the invoking pane, never the popup.
    if (this.opening?.space) env.push(`HERDR_WORKSPACE_ID=${this.opening.space}`);
    if (this.opening?.tab) env.push(`HERDR_TAB_ID=${this.opening.tab}`);
    if (this.opening?.terminal) env.push(`HERDR_PANE_ID=${this.opening.terminal}`);
    // Load the same login/interactive environment as the existing terminal TUI.
    // All user values stay argv entries; the shell only forwards "$@".
    const result = await exec("zsh", ["-lic", 'exec "$@"', "berbr-env", "env", ...env, this.binary,
      ...(this.project ? ["--project", this.project] : []), ...args], this.cwd ? { cwd:this.cwd } : {});
    if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || `berbr exited ${result.code}`);
    return result.stdout;
  }

  refresh(cx, cached = false) {
    if (this.loading || this.busy) return;
    this.loading = true;
    cx.spawn(async (cx) => {
      try {
        const data = JSON.parse(await this.cli(["app-snapshot", ...(cached ? ["--cached"] : [])]));
        this.project = data.project;
        this.agent = data.projects.find(p => p.name === data.project)?.agent ?? "codex";
        setShared("berbr.project", this.project);
        this.data = data; this.error = null;
        if (!this.selected) {
          const flat = walk(data.rows);
          const caller = flat.find(r => r.terminal === this.opening?.terminal)
            ?? flat.find(r => r.space === this.opening?.space);
          const task = caller?.task;
          this.selected = flat.find(r => r.kind === "task" && r.task === task)?.id
            ?? flat.find(r => r.kind !== "header")?.id;
          for (const row of data.rows) if (row.kind === "task" && row.id !== this.selected) this.folded.add(row.id);
        }
      } catch (error) { this.error = String(error.message ?? error); }
      this.loading = false; cx.notify();
      // First paint uses the cache; ask the services after the native view is ready.
      if (cached && !this.didOnline) { this.didOnline = true; this.refresh(cx); }
    });
  }

  switchProject(name, cx) {
    if (this.loading || this.busy || name === this.project) return;
    this.project = name; this.selected = null; this.folded.clear(); this.didOnline = false;
    this.cwd = this.data?.projects.find(p => p.name === name)?.cwd ?? null;
    this.data = null; this.refresh(cx, true);
  }

  rows() {
    const query = this.filter.trim().toLowerCase();
    const matches = r => `${r.label ?? ""} ${r.state ?? ""} ${(r.tags ?? []).join(" ")}`.toLowerCase().includes(query)
      || (r.children ?? []).some(matches);
    const rows = [];
    const add = (r, depth = 0) => {
      if (!this.all && (r.done || r.state === "finished")) return;
      if (query && !matches(r)) return;
      rows.push({ ...r, depth });
      if (query || !this.folded.has(r.id)) for (const child of r.children ?? []) add(child, depth + 1);
    };
    for (const row of this.data?.rows ?? []) add(row);
    return rows;
  }

  chosen() { return walk(this.data?.rows ?? []).find(r => r.id === this.selected); }

  perform(cx, args, {close = false} = {}) {
    if (this.busy || this.loading) return;
    this.busy = true; this.error = null;
    cx.spawn(async (cx) => {
      try {
        const result = await this.cli(args);
        this.busy = false;
        run("berbr.refresh");
        if (close && props()?.overlay) closeOverlay(props().overlay);
        notify({title:"Berbr",body:result.trim() || "Done",level:"info"});
        if (!this.form) this.refresh(cx, true);
      } catch (error) {
        this.error = String(error.message ?? error); this.busy = false;
      }
      cx.notify();
    });
  }

  open(row, cx) {
    if (!row || this.busy) return;
    if (row.kind === "agent") {
      run("agent.focus", {terminal:row.terminal});
      if (props()?.overlay) closeOverlay(props().overlay);
    } else if (row.kind === "mr" || row.kind === "ticket") {
      if (row.url) cx.spawn(async () => { await exec("open", [row.url]); });
    } else if (row.kind === "task" || row.kind === "tree") {
      const trees = walk(row.children ?? []).filter(r => r.kind === "tree");
      if (row.kind === "task" && trees.length > 1) {
        this.formOverlay("detail",{row}); return;
      }
      const args = ["select", row.task ?? row.tree ?? row.target];
      const tree = row.tree ?? trees[0]?.tree;
      if (tree) args.push("--tree", tree);
      this.perform(cx, args, {close:true});
    }
  }

  formOverlay(form, extra = {}) {
    openOverlay({mod:"berbr",kind:"dialog",width:form === "detail" ? 820 : 580,height:form === "detail" ? 0.85 : form === "new" ? 510 : 260,
      props:{form,project:this.project,binary:this.binary,agent:this.agent,...extra}});
  }

  submitNew(cx) {
    const title = this.values.title?.trim();
    const args = ["start", ...(title ? [title] : ["--unnamed"]), "--bg"];
    if (this.agent === "none") args.push("--no-agent"); else args.push("--agent",this.agent);
    if (!this.worktree) args.push("--no-worktree");
    if (this.newTicket) args.push("--new-ticket");
    const ticket = this.values.ticket?.trim();
    if (ticket) args.push("--ticket",ticket);
    const branch = this.values.branch?.trim();
    if (branch) args.push("--branch",branch);
    const name = this.values.name?.trim();
    if (name) args.push("--name",name);
    this.perform(cx,args,{close:true});
  }

  key(e, cx) {
    if (e.key === "escape" && props()?.overlay) {
      if (!this.busy) closeOverlay(props().overlay);
      cx.stop_propagation(); return;
    }
    if (this.form) return;
    const query = this.filter.trim().toLowerCase();
    const rows = walk(this.data?.rows ?? []).filter(r=>r.kind === "task" && (this.all || !r.done))
      .filter(r=>!query || r.label.toLowerCase().includes(query));
    const index = rows.findIndex(r=>r.id === this.selected);
    if (e.key === "down" || e.key === "up") {
      const delta = e.key === "down" ? 1 : -1;
      this.selected = rows[Math.max(0,Math.min(rows.length-1,index+delta))]?.id;
      cx.stop_propagation(); cx.notify();
    } else if (e.key === "enter") {
      const row = rows.find(r=>r.id === this.selected);
      if (row) this.taskClick(row,cx);
      cx.stop_propagation();
    }
  }

  button(id, label, action, primary = false, disabled = false) {
    const t = theme();
    return h_flex().id(id).h(30).px(11).gap(6).items_center().rounded(6)
      .bg(primary ? t.solid : t.raised).text_color(primary ? t.solid_foreground : t.foreground)
      .text_size(12).cursor_pointer().opacity(disabled ? 0.4 : 1)
      .hover(s => s.opacity(disabled ? 0.4 : 0.8))
      .on_click((_e,cx) => { if (!disabled) action(cx); }).child(label);
  }

  input(key, placeholder) {
    const t = theme();
    return Input.new(this.inputs[key]).h(34).px(10).rounded(7)
      .border_1().border_color(t.input).bg(t.background).text_color(t.foreground).text_size(13);
  }

  formView(cx) {
    const t = theme();
    const p = props() ?? {};
    const label = text => div().text_size(11).text_color(t.muted).pt(8).child(text);
    const root = v_flex().size_full().p(20).gap(8).bg(t.popover).text_color(t.foreground);
    if (this.form === "detail") return this.detailView(cx);
    if (this.form === "ticket") return root
      .child(div().text_size(18).font_semibold().child("Add Jira ticket"))
      .child(div().text_size(12).text_color(t.muted).child(p.task))
      .child(this.input("ticket", "DEMO-123"))
      .child(h_flex().gap(8)
        .child(this.button("add-tree", "Worktree + agent", cx=>this.perform(cx,["add",p.task,this.values.ticket ?? ""],{close:true}),true,this.busy))
        .child(this.button("attach", "Attach only", cx=>this.perform(cx,["attach",this.values.ticket ?? "",p.task],{close:true}),false,this.busy)))
      .when(Boolean(this.error),el=>el.child(div().text_color(t.blocked).child(this.error)));
    if (this.form === "new") {
      root.child(div().text_size(18).font_semibold().child("New task"))
        .child(div().text_size(12).text_color(t.muted).child(this.project ?? ""))
        .child(label("Title · leave empty for the agent to name it"))
        .child(this.input("title","Task title"))
        .child(h_flex().gap(8).children(["codex","claude","none"].map(agent =>
          this.button(`agent-${agent}`,agent, cx => {this.agent=agent;cx.notify();},this.agent===agent))))
        .child(h_flex().gap(8)
          .child(this.button("worktree",`${this.worktree ? "☑" : "☐"} Worktree`,cx=>{this.worktree=!this.worktree;cx.notify();}))
          .child(this.button("new-ticket",`${this.newTicket ? "☑" : "☐"} New Jira ticket`,cx=>{this.newTicket=!this.newTicket;cx.notify();})))
        .child(label("Existing Jira ticket" )).child(this.input("ticket","Optional · DEMO-123"))
        .child(h_flex().gap(8)
          .child(v_flex().flex_1().gap(4).child(label("Worktree name")).child(this.input("name","Automatic")))
          .child(v_flex().flex_1().gap(4).child(label("Branch")).child(this.input("branch","Automatic"))));
    } else {
      root.child(div().text_size(18).font_semibold().child(p.title ?? "Close task?"))
        .child(div().text_size(13).child(p.label ?? ""))
        .child(div().text_size(12).text_color(t.muted).child("Removes eligible worktrees, closes your Jira tickets and marks the task done. Existing checks still apply."));
    }
    return root.child(div().flex_1())
      .when(Boolean(this.error),el=>el.child(div().text_size(12).text_color(t.blocked).child(this.error)))
      .child(h_flex().gap(8).justify_end()
        .child(this.button("cancel","Cancel",()=>closeOverlay(p.overlay),false,this.busy))
        .child(this.button("submit",this.busy ? "Working…" : this.form === "new" ? "Create task" : "Close task",
          cx => this.form === "new" ? this.submitNew(cx) : this.perform(cx,p.args ?? ["done",p.task,"--yes"],{close:true}),true,this.busy)));
  }

  detailView(cx) {
    const t = theme(), row = props().row;
    const related = walk(row.children ?? []);
    const trees = related.filter(r=>r.kind === "tree");
    const busy = this.busy;
    const root = v_flex().size_full().min_h(0).p(24).gap(14).bg(t.popover).text_color(t.foreground);
    root.child(h_flex().gap(10).items_center()
      .child(div().flex_1().text_size(21).font_semibold().child(row.title ?? row.label))
      .child(this.button("close-details","×",()=>closeOverlay(props().overlay))))
      .child(h_flex().gap(8)
        .child(div().flex_1().text_size(12).text_color(tones(t,row.state)).child(`${row.task} · ${row.state ?? "idea"}`))
        .when(Boolean(row.space),el=>el.child(this.button("go-space","Go to space",()=>{run("space.select",{id:row.space});closeOverlay(props().overlay);},true)))
        .when(!row.space && trees.length <= 1,el=>el.child(this.button("continue","Start work",cx=>this.open(row,cx),true,busy)))
        .when(Boolean(row.note),el=>el.child(this.button("open-note","Note",cx=>cx.spawn(async()=>{await exec("open",[row.note]);}))))
        .child(this.button("add-ticket","+ Ticket",()=>this.formOverlay("ticket",{task:row.task}),false,busy)));
    const body = v_flex().flex_1().min_h(0).gap(10).overflow_y_scrollbar();
    if (row.body) body.child(TextView.markdown(`detail-${row.id}`,row.body).text_size(13));
    for (const child of related) {
      const line = h_flex().gap(8).items_center().py(4)
        .child(div().w(16).text_color(tones(t,child.state)).child(ICON[child.kind] ?? "·"))
        .child(div().flex_1().min_w(0).text_size(12).truncate().child(child.label))
        .child(div().text_size(10.5).text_color(t.dim).child(child.state ?? ""));
      if (child.kind === "tree") line
        .child(this.button(`go-${child.id}`,"Continue",cx=>this.open(child,cx),false,busy))
        .child(this.button(`review-${child.id}`,"Review",cx=>this.perform(cx,["review",child.tree]),false,busy));
      if (child.kind === "agent") line.child(this.button(`agent-${child.id}`,"Go",cx=>this.open(child,cx)));
      if (child.url) line.child(this.button(`link-${child.id}`,"↗",cx=>this.open(child,cx)));
      body.child(line);
    }
    return root.child(body)
      .when(Boolean(this.error),el=>el.child(div().text_size(12).text_color(t.blocked).child(this.error)))
      .child(h_flex().justify_between().items_center()
        .child(div().flex_1().min_w(0).text_size(10.5).text_color(t.dim).truncate().child(row.note ?? ""))
        .when(!row.done,el=>el.child(this.button("done","Close task",()=>this.formOverlay("done",{task:row.task,label:row.label}),false,busy))));
  }

  taskClick(row, cx) {
    this.selected = row.id; cx.notify();
    if (row.space && spaces().some(s=>s.id === row.space)) run("space.select",{id:row.space});
    else this.formOverlay("detail",{row});
  }

  render(cx) {
    if (this.form) return this.formView(cx);
    const t = theme();
    const live = new Map(agents().map(a=>[a.address,a]));
    const disabled = this.loading || this.busy;
    const query = this.filter.trim().toLowerCase();
    const rows = walk(this.data?.rows ?? []).filter(r=>r.kind === "task")
      .filter(r=>this.all || !r.done)
      .filter(r=>!query || `${r.label} ${r.state ?? ""} ${(r.tags ?? []).join(" ")} ${walk(r.children ?? []).map(c=>c.label).join(" ")}`.toLowerCase().includes(query));
    const list = v_flex().id("berbr-tasks").flex_1().min_h(0).overflow_y_scrollbar().gap(3).px(8).py(6);
    for (const row of rows) {
      const taskAgents = walk(row.children ?? []).filter(r=>r.kind === "agent").map(r=>live.get(r.terminal)).filter(Boolean);
      const attention = taskAgents.find(a=>a.status === "blocked") ?? taskAgents.find(a=>a.status === "done");
      const working = taskAgents.filter(a=>a.status === "working").length;
      const active = row.id === this.selected || (row.space && spaces().some(s=>s.id === row.space && s.active));
      const hint = attention ? (attention.status === "blocked" ? "Needs you" : "Finished") : working ? `${working} working` : "";
      list.child(h_flex().id(`task-${row.id}`).min_h(58).px(10).py(9).gap(8).items_center().rounded(7)
        .bg(active ? t.selected : t.surface).hover(s=>s.bg(t.hover)).cursor_pointer()
        .on_click((_e,cx)=>this.taskClick(row,cx))
        .child(div().size(7).flex_none().rounded(4).bg(tones(t,attention?.status ?? (working ? "working" : row.state))))
        .child(v_flex().flex_1().min_w(0).gap(4)
          .child(div().text_size(12.5).text_color(t.foreground).truncate().child(row.title ?? row.label))
          .child(h_flex().gap(7).items_center().text_size(10.5)
            .when(row.task !== row.title,el=>el.child(div().max_w(100).truncate().text_color(t.dim).child(row.task)))
            .child(div().text_color(tones(t,row.state)).child(row.state ?? "idea"))
            .when(Boolean(hint),el=>el.child(div().text_color(tones(t,attention?.status ?? "working")).child(hint)))))
        .child(div().id(`details-${row.id}`).w(24).h(28).flex_none().flex().items_center().justify_center().rounded(5)
          .text_size(15).text_color(t.dim).hover(s=>s.bg(t.raised).text_color(t.foreground))
          .on_click((_e,cx)=>{cx.stop_propagation();this.formOverlay("detail",{row});}).child("⋯")));
    }
    if (!rows.length) list.child(div().p(14).text_size(12).text_color(t.dim).child(this.loading ? "Loading tasks…" : "No matching tasks"));
    return v_flex().id("berbr").track_focus(this.handle).on_key_down((e,cx)=>this.key(e,cx))
      .size_full().min_h(0).min_w(0).bg(t.surface).text_color(t.foreground)
      .child(h_flex().h(44).px(14).gap(8).items_center().border_b_1().border_color(t.border)
        .child(div().flex_1().text_size(14).font_semibold().child("Berbr"))
        .child(this.button("all",this.all?"All":"Open",cx=>{this.all=!this.all;cx.notify();}))
        .child(this.button("refresh",this.loading?"…":"↻",cx=>this.refresh(cx),false,disabled))
        .child(this.button("new","+",()=>this.formOverlay("new"),true,disabled)))
      .child(h_flex().px(10).py(8).gap(7).items_center()
        .child(this.button("project",`${this.project ?? "Project"} ▾`,cx=>{this.projectMenu=!this.projectMenu;cx.notify();},false,disabled))
        .child(div().flex_1().min_w(0).child(this.input("search","Search tasks…"))))
      .when(this.projectMenu,el=>el.child(v_flex().px(10).gap(3).pb(8).children((this.data?.projects ?? []).map(p=>
        this.button(`project-${p.name}`,p.name,cx=>{this.projectMenu=false;this.switchProject(p.name,cx);},p.name===this.project,disabled)))))
      .when(Boolean(this.error),el=>el.child(div().px(12).py(8).text_size(11).text_color(t.blocked).child(this.error)))
      .child(list)
      .child(h_flex().h(28).px(12).gap(8).items_center().border_t_1().border_color(t.border).text_size(10).text_color(t.dim)
        .child(div().child(`${rows.length} tasks`)).child(div().flex_1())
        .child(div().child(this.loading ? "Refreshing…" : `${this.data?.sources?.jira ?? "…"} Jira · ${this.data?.sources?.gitlab ?? "…"} GitLab`)))
      .when(Boolean(this.data?.warnings?.length),el=>el.child(div().px(12).py(6).text_size(10.5).text_color(t.blocked).child(this.data.warnings.join(" · "))));
  }
}
