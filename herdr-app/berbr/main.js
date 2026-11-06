import { View, div, svg, image } from "gpui-kit";
import { h_flex, v_flex, Input, InputState, TextView } from "gpui-base";
import { appHome, exec, focused, spaces, agents, panes, theme, run, props,
  openOverlay, closeOverlay, notify, shared, setShared, nextCommand, hc } from "herdr";
import { walk, sectionLabels, taskSpace, rowMenu,
  normalizeTicketState, ticketTitle,
  cleanCliText, isClosedTicket,
  agentActivity, formatElapsed, blockedPrompt, mrPills, focusedTask, focusedTree, treeFocusTarget, jumpItems, matchJump, closePlan, PanelModel, treeMarks, isMergedTree, mergedPlan, backendClosePlan } from "./model.js";
import { icon, iconButton, button, chip, sectionHeader, statusMark, kbd, pill } from "./ui.js";
import { formView, initNote, noteKey } from "./forms.js";

export default class Berbr extends View {
  init(_props, cx) {
    const p = props() ?? {};
    this.binary = p.binary ?? `${appHome()}/mods/berbr/berbr`;
    this.form = p.form ?? null;
    this.project = p.project ?? shared("berbr.project") ?? null;
    this.pendingProject = null;
    this.data = null; this.error = null; this.busy = false; this.loading = false;
    this.filter = ""; this.all = false; this.selected = null; this.hovered = null;
    this.expanded = null; this.busyAction = null;
    this.blockedReasons = new Map(); this.panelModel = new PanelModel();
    this.promptQueue = []; this.promptWorkers = 0;
    this.jumpIndex = 0; this.jumpQuery = "";
    this.menuIndex = 0; this.pickerIndex = 0;
    if (this.form === "menu") this.menuItems = rowMenu(p.row, {spaces:spaces(), mr:p.mr, live:p.live});
    if (this.form === "done") this.closePlan = p.plan ?? closePlan(p.row, spaces());
    if (this.form === "remove-merged") this.bulkPlan = mergedPlan(p.trees);
    this.history = new Set(); this.subtasks = new Set(); this.closedTickets = new Set();
    try {
      const saved = JSON.parse(localStorage.getItem("berbr.expanded"));
      if (typeof saved === "string") this.expanded = saved;
    } catch (_) { /* Storage is optional. */ }
    this.collapsed = new Set(["cleanup"]);
    try {
      const saved = JSON.parse(localStorage.getItem("berbr.collapsed"));
      if (Array.isArray(saved)) {
        this.collapsed = new Set(saved.map(id => id === "unsorted" ? "cleanup" : id === "ideas" ? "todo" : id)
          .filter(id => Object.keys(sectionLabels).includes(id)));
        if (JSON.stringify(saved) !== JSON.stringify([...this.collapsed]))
          localStorage.setItem("berbr.collapsed", JSON.stringify([...this.collapsed]));
      }
    } catch (_) { /* Storage is optional. */ }
    this.fixedContext = Boolean(this.form) || p.opening !== undefined;
    this.opening = p.opening !== undefined ? p.opening : focused();
    this.currentFocus = focused();
    this.cwd = p.cwd ?? this.opening?.cwd ?? null;
    this.handle = cx.focus_handle();
    this.handle.focus();
    if (this.form === "note") { initNote(this, cx); return; }
    this.inputs = {}; this.values = {}; this.searchFocused = false;
    const placeholders = { search: this.form === "jump" ? "Jump to…" : "Filter tasks", title: "What are you working on?",
      ticket: "DEMO-123", branch: "Automatic", name: "Automatic" };
    for (const key of Object.keys(placeholders)) {
      const value = p.prefill?.[key] ?? "";
      const input = InputState.new({ value, placeholder: placeholders[key] });
      this.values[key] = value;
      input.on("change", (_event, cx) => {
        this.values[key] = input.value();
        if (key === "search") this.filter = input.value();
        if (key === "search") this.pickerIndex = 0;
        if (key === "search" && this.form === "jump") { this.jumpQuery = input.value(); this.jumpIndex = 0; }
        cx.notify();
      });
      input.on("submit", (_event, cx) => {
        if (this.form === "jump" && key === "search") this.chooseJump(cx);
        if (["projects", "attach-task"].includes(this.form) && key === "search") this.choosePicker(cx);
        if (this.form === "new" && ["title", "ticket"].includes(key)) this.submitNew(cx);
        if (this.form === "rename" && key === "title") this.submitRename(cx);
        if (this.form === "ticket" && key === "ticket") this.submitTicket(cx, true);
      });
      if (key === "search") {
        input.on("focus", (_event, cx) => { this.searchFocused = true; cx.notify(); });
        input.on("blur", (_event, cx) => { this.searchFocused = false; cx.notify(); });
      }
      this.inputs[key] = input;
    }
    this.agent = p.agent ?? "codex"; this.worktree = true;
    this.ticketMode = p.prefill?.ticket ? "existing" : "none";
    this.advanced = false;
    if (!this.form) {
      this.refresh(cx, true);
      cx.timer.every(1000, cx => {
        this.syncFocus(cx);
        const live = this.liveState();
        this.syncBlocked(live.rows, cx, false, live);
      });
      cx.timer.every(30000, cx => {
        this.withLive(live => {
          this.refresh(cx, true);
          if (live.rows.some(row => row.kind === "task" && live.signal(row).working)) cx.notify();
        });
      });
      cx.timer.every(300000, cx => this.refresh(cx));
      cx.spawn(async cx => {
        for (;;) {
          const command = await nextCommand();
          if (command === "refresh") this.refresh(cx, true);
          if (command === "new") this.formOverlay("new");
          if (command === "details") this.withLive(() => { const row = this.chosen(); if (row) this.showDetails(row, cx); });
          if (command === "focus") { this.handle.focus(); this.receiveJump(cx); }
          if (command === "jump") this.openJump();
          if (command === "navigate") this.receiveJump(cx);
          cx.notify();
        }
      });
    } else if (this.form === "jump") {
      this.jumpEntries = jumpItems(p.snapshot, p.agents ?? []); this.inputs.search.focus();
    } else if (["projects", "attach-task"].includes(this.form)) {
      this.inputs.search.focus();
    } else if (["new", "rename"].includes(this.form)) this.inputs.title.focus();
    else if (this.form === "ticket") this.inputs.ticket.focus();
    if (["done", "remove-merged"].includes(this.form)) this.loadClosePlan(cx);
  }

  actionContext() {
    const opening = this.fixedContext ? this.opening : focused();
    const projectCwd = this.data?.projects?.find(project => project.name === this.project)?.cwd;
    return {opening, cwd:this.fixedContext ? this.cwd : opening?.cwd ?? projectCwd ?? this.cwd};
  }

  syncFocus(cx) {
    const next = focused(), previous = this.currentFocus;
    if (["space", "tab", "terminal", "cwd"].every(key => next?.[key] === previous?.[key])) return;
    this.currentFocus = next;
    if (!this.panelFocused() && ["space", "terminal"].some(key => next?.[key] !== previous?.[key])) {
      const task = focusedTask(this.data?.rows, next, spaces(), agents());
      if (task) this.selected = this.followedSelection = task.id;
      // Plain overflow containers expose no programmatic scroll handle in this API.
    }
    cx.notify();
  }

  async cli(args) {
    const env = ["-u", "HERDR_PLUGIN_CONTEXT_JSON", "-u", "HERDR_PLUGIN_ROOT", "-u", "HERDR_ENV", "-u", "HERDR_WORKSPACE_ID", "-u", "HERDR_TAB_ID", "-u", "HERDR_PANE_ID", "-u", "HC_TERMINAL_ID", "BERBR_BACKEND=herdr-app", "NO_COLOR=1"];
    const {opening, cwd} = this.actionContext();
    // Clear inherited caller ids too: a mod pane can have no terminal.
    if (opening?.space) env.push(`HERDR_WORKSPACE_ID=${opening.space}`);
    if (opening?.tab) env.push(`HERDR_TAB_ID=${opening.tab}`);
    if (opening?.terminal) env.push(`HERDR_PANE_ID=${opening.terminal}`);
    // Load the same login/interactive environment as the existing terminal TUI.
    // All user values stay argv entries; the shell only forwards "$@".
    const result = await exec("zsh", ["-lic", 'exec "$@"', "berbr-env", "env", ...env, this.binary,
      ...(this.project ? ["--project", this.project] : []), ...args], cwd ? {cwd} : {});
    if (result.code !== 0) throw new Error(cleanCliText(result.stderr) || cleanCliText(result.stdout) || `berbr exited ${result.code}`);
    return result.stdout;
  }

  refresh(cx, cached = false, forcePrompts = false) {
    if (this.loading || this.busy) return;
    const desired = shared("berbr.project");
    if (desired && desired !== this.project) { this.switchProject(desired, cx); return; }
    this.loading = true;
    const live = this.liveState();
    this.syncBlocked(live.rows, cx, forcePrompts, live);
    const requestedProject = this.project;
    cx.spawn(async cx => {
      try {
        const data = JSON.parse(await this.cli(["app-snapshot", ...(cached ? ["--cached"] : [])]));
        const desired = shared("berbr.project");
        if (desired && desired !== requestedProject && desired !== data.project) {
          this.loading = false; this.switchProject(desired, cx); return;
        }
        this.project = data.project;
        this.agent = data.projects?.find(project => project.name === data.project)?.agent ?? "codex";
        setShared("berbr.project", this.project);
        const firstData = !this.data;
        this.data = data; this.contextRow = null; this.error = null;
        if (firstData) {
          const flat = walk(data.rows);
          const task = focusedTask(data.rows, this.actionContext().opening, spaces(), agents());
          this.selected = task?.id ?? this.expanded;
          if (task) this.setExpanded(task.id, cx);
          if ((task ?? flat.find(row => row.id === this.expanded))?.done) this.all = true;
        }
      } catch (error) { this.error = String(error.message ?? error); }
      this.loading = false;
      const live = this.liveState();
      this.syncBlocked(live.rows, cx, false, live); cx.notify();
      if (cached && !this.didOnline) { this.didOnline = true; this.refresh(cx); }
    });
  }

  switchProject(name, cx) {
    if (this.loading || this.busy || name === this.project) return;
    this.project = name; this.selected = null; this.didOnline = false;
    this.contextRow = null;
    this.setExpanded(null, cx);
    this.cwd = this.data?.projects?.find(project => project.name === name)?.cwd ?? null;
    this.data = null; this.refresh(cx, true);
  }

  liveState() {
    if (this.renderLive) return this.renderLive;
    return this.panelModel.update(this.data, agents(), spaces(), {
      query:this.filter, showDone:this.all, collapsed:this.collapsed, expanded:this.expanded,
    });
  }

  withLive(action) {
    const previous = this.renderLive;
    this.renderLive = this.liveState();
    try { return action(this.renderLive); }
    finally { this.renderLive = previous; }
  }

  signal(row) { return this.liveState().signal(row); }

  activity(agent, treeName) {
    if (!agent) return {text:"", elapsed:""};
    const live = this.liveState();
    live.activities ??= new Map();
    const key = agent.address;
    if (!live.activities.has(key) || live.activities.get(key)?.seq !== agent.seq || live.activities.get(key)?.treeName !== treeName) {
      const tab = live.spaces.find(space => space.id === agent.space)?.tabs?.find(tab => tab.id === agent.tab);
      live.activities.set(key, {seq:agent.seq, treeName, text:agentActivity(agent.title, agent.app ?? agent.program?.name, treeName),
        since:tab?.working_since});
    }
    const activity = live.activities.get(key);
    return {text:activity.text, elapsed:formatElapsed(activity.since == null ? null : Date.now() - activity.since)};
  }

  clearBlocked(address, cx) {
    this.blockedReasons.delete(address);
    if (this.replyFocused?.endsWith(`/waiting/${address}`)) {
      this.replyFocused = null; this.handle.focus(); cx.notify();
    }
  }

  syncBlocked(rows, cx, force = false, live = this.liveState()) {
    for (const [address, entry] of this.blockedReasons) {
      const agent = live.live.get(address);
      if (!agent || agent.status !== "blocked" || (!entry.busy && agent.seq !== entry.seq))
        this.clearBlocked(address, cx);
    }
    // Read the expanded/selected task first, then the other displayed tasks.
    const tasks = rows.filter(row => row.kind === "task");
    const priority = tasks.filter(row => row.id === this.expanded || row.id === this.selected);
    const blocked = new Map([...priority, ...tasks].flatMap(row => live.signal(row).agents)
      .filter(agent => agent.status === "blocked" && agent.address).map(agent => [agent.address, agent]));
    // An automatic snapshot arriving mid-read must not cancel a manual refresh pass.
    if (force || !this.promptScan || !this.promptForce) {
      this.promptScan = blocked.values(); this.promptForce = force;
    }
    this.fillPromptQueue();
    while (this.promptWorkers < 4 && this.promptWorkers < this.promptQueue.length) {
      this.promptWorkers++;
      cx.spawn(async cx => {
        try {
          for (;;) {
            this.fillPromptQueue();
            const job = this.promptQueue.shift();
            if (!job) break;
            const {address, entry} = job;
            const latest = agents().find(agent => agent.address === address);
            if (this.blockedReasons.get(address) !== entry || latest?.status !== "blocked" || latest.seq !== entry.seq) continue;
            try {
              const [info, screen] = await Promise.all([hc(["get", address]), hc(["read", address])]);
              let status;
              try { if (info.code === 0) status = JSON.parse(info.stdout).status; } catch (_) { /* Older cores may not report a message. */ }
              Object.assign(entry, blockedPrompt(status, screen.code === 0 ? screen.stdout : ""));
            } catch (_) { /* An unreadable prompt still needs the person. */ }
            const current = agents().find(agent => agent.address === address);
            if (this.blockedReasons.get(address) !== entry || current?.status !== "blocked" || current.seq !== entry.seq) continue;
            entry.loading = false; entry.readAt = Date.now(); cx.notify();
          }
        } finally { this.promptWorkers--; }
      });
    }
  }

  fillPromptQueue() {
    while (this.promptScan && this.promptQueue.length < 64) {
      const {value:agent, done} = this.promptScan.next();
      if (done) { this.promptScan = null; break; }
      const previous = this.blockedReasons.get(agent.address);
      if (previous?.busy || previous?.loading || (!this.promptForce && previous &&
          previous.seq === agent.seq && Date.now() - previous.readAt < 300000)) continue;
      const entry = {...previous, seq:agent.seq, reason:previous?.reason ?? {kind:"waiting"},
        choices:previous?.choices ?? [], loading:true};
      this.blockedReasons.set(agent.address, entry);
      this.promptQueue.push({address:agent.address, entry});
    }
  }

  waitingItems(task) {
    if (task.done) return [];
    return this.signal(task).agents.filter(agent => agent.status === "blocked" && agent.address).map(agent => ({
      id:`${task.id}/waiting/${agent.address}`, kind:"waiting", parent:task, agent,
    }));
  }

  replyTo(item, cx) {
    const entry = this.blockedReasons.get(item.agent.address);
    if (!entry || entry.busy || this.busy) return;
    this.selected = item.id;
    if (!entry.input) {
      entry.input = InputState.new({value:"", placeholder:"Reply to agent"});
      entry.input.on("submit", (_event, cx) => this.answerAgent(item, entry.input.value(), cx, true));
      entry.input.on("focus", () => { this.replyFocused = item.id; });
      entry.input.on("blur", () => { this.replyFocused = null; });
    }
    entry.reply = true; entry.input.focus(); cx.notify();
  }

  answerAgent(item, value, cx, reply = false) {
    const address = item.agent.address, entry = this.blockedReasons.get(address);
    const current = agents().find(agent => agent.address === address);
    if (!entry || entry.busy || entry.loading || this.busy || !String(value).trim() ||
        current?.status !== "blocked" || current.seq !== entry.seq ||
        (!reply && !entry.choices.some(choice => choice.key === value))) return;
    entry.busy = true; entry.feedback = null; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await hc([reply ? "send" : "keys", address, value]);
        if (result.code !== 0) throw new Error(result.stderr?.trim() || "Could not answer agent");
        await cx.sleep(1000);
        const info = await hc(["get", address]);
        if (info.code !== 0) throw new Error("Could not check answer — go to agent");
        const status = JSON.parse(info.stdout).status;
        if (!status?.state || (status.state === "blocked" && typeof status.msg !== "string"))
          throw new Error("Could not check answer — go to agent");
        if (this.blockedReasons.get(address) !== entry) return;
        if (status?.state === "blocked" && status.msg === entry.msg) {
          entry.seq = agents().find(agent => agent.address === address)?.seq;
          entry.feedback = "Not answered — go to agent";
        } else if (status?.state === "blocked") {
          const screen = await hc(["read", address]);
          Object.assign(entry, blockedPrompt(status, screen.code === 0 ? screen.stdout : ""));
          entry.seq = agents().find(agent => agent.address === address)?.seq;
          entry.reply = false;
        } else this.blockedReasons.delete(address);
        if (reply && entry.input.value() === value) entry.input.set_value("");
      } catch (error) { entry.feedback = String(error.message ?? error); }
      finally {
        entry.busy = false;
        if (reply && this.replyFocused === item.id) { this.replyFocused = null; this.handle.focus(); }
        cx.notify();
      }
    });
  }

  waitingCard(t, item) {
    const entry = this.blockedReasons.get(item.agent.address);
    if (!entry || (!entry.question && !entry.feedback)) return div();
    const disabled = this.busy || entry.busy || entry.loading;
    const selected = this.panelFocused() && this.selected === item.id;
    const small = (id, label, action) => button(t, `${item.id}/${id}`, label, action, "ghost", disabled)
      .h(24).px(6).min_w(0).max_w("100%").truncate();
    return v_flex().id(item.id).mx(8).ml(28).mb(4).p(6).gap(4).min_w(0).rounded(6)
      .border_1().border_color(selected ? t.accent : t.border)
      .on_click((_event, cx) => {cx.stop_propagation(); this.selected = item.id; this.handle.focus(); cx.notify();})
      .child(div().text_size(12).text_color(t.foreground).line_height(1.35).max_h(49).overflow_hidden()
        .whitespace_normal().child(entry.question ?? ""))
      .child(h_flex().gap(2).min_w(0).flex_wrap().children(entry.choices.map(choice =>
        small(choice.key, `${choice.key} ${choice.label.length > 30 ? choice.label.slice(0, 29) + "…" : choice.label}`,
          cx => this.answerAgent(item, choice.key, cx)).tooltip(choice.label)))
        .child(small("reply", "Reply", cx => this.replyTo(item, cx)))
        .child(small("focus", "Go to agent", () => run("agent.focus", {terminal:item.agent.address}))))
      .when(Boolean(entry.reply), el => el.child(div().min_w(0)
        .on_click((_event, cx) => cx.stop_propagation())
        .child(Input.new(entry.input).h(26).w_full().min_w(0).px(6)
          .text_size(12).bg(t.background).text_color(t.foreground))))
      .when(Boolean(entry.feedback), el => el.child(div().text_size(11).text_color(t.dim).whitespace_normal().child(entry.feedback)));
  }

  attention(agent) {
    if (agent?.status === "done") return "Finished · review";
    const reason = this.blockedReasons.get(agent?.address)?.reason;
    if (reason?.kind === "permission") return `Asks permission${reason.detail ? ` · ${reason.detail}` : ""}`;
    if (reason?.kind === "question") return `Asks: ${reason.detail}`;
    return "Waiting for you";
  }

  sections() {
    return this.liveState().visible;
  }

  rows(sections = null) { return this.withLive(() => this.rowItems(sections ?? this.sections())); }

  rowItems(sections) {
    return sections.flatMap(section => [
      {id:`section-${section.id}`, kind:"section", section:section.id},
      ...(section.expanded ? section.rows.flatMap(row => {
        const waiting = row.kind === "task" ? this.waitingItems(row).filter(item => this.blockedReasons.get(item.agent.address)?.question) : [];
        const details = row.id === this.expanded
          ? (row.kind === "task" ? this.detailItems(row) : this.looseItems(row)).filter(item => item.action) : [];
        if (!details.length) return [row, ...waiting];
        return [row, ...details.flatMap(item => [item, ...waiting.filter(prompt => item.type === "agent" && item.live?.address === prompt.agent.address)]),
          ...waiting.filter(prompt => !details.some(item => item.type === "agent" && item.live?.address === prompt.agent.address))];
      }) : []),
    ]);
  }
  chosen() { return this.rows().find(row => row.id === this.selected)
    ?? (this.contextRow?.id === this.selected ? this.contextRow : undefined); }

  openJump() {
    if (!this.busy) this.formOverlay("jump", {snapshot:this.data, agents:agents()});
  }

  jumpMatches() { return matchJump(this.jumpEntries ?? [], this.jumpQuery); }

  pickerMatches() {
    const p = props() ?? {}, query = this.filter.trim().toLowerCase();
    return (this.form === "projects" ? p.projects ?? [] : p.tasks ?? [])
      .filter(item => [item.name, item.title, item.task].filter(Boolean).join(" ").toLowerCase().includes(query));
  }

  choosePicker(cx, item = this.pickerMatches()[this.pickerIndex]) {
    if (!item || this.busy || this.pickerChosen) return;
    if (this.form === "projects") {
      this.pickerChosen = true;
      setShared("berbr.project", item.name); closeOverlay(props().overlay);
    } else {
      if (!props()?.ticket || !item.task) { this.error = "Ticket or task name is unavailable."; cx.notify(); return; }
      this.perform(cx, ["attach", props().ticket, item.task], {close:true, action:`attach-task-${item.id}`});
    }
  }

  chooseJump(cx, item = this.jumpMatches()[this.jumpIndex]) {
    if (!item || this.jumpChosen) return;
    this.jumpChosen = true;
    if (item.kind === "project") setShared("berbr.project", item.project.name);
    else if (item.kind === "agent") run("agent.focus", {terminal:item.live?.address ?? item.row.terminal});
    else if (["ticket", "mr"].includes(item.kind) && !item.task) this.openUrl(item.row.url, cx);
    else {
      setShared("berbr.jumpTarget", {project:this.project, task:item.task?.id, row:item.row.id,
        parent:item.parent?.id, kind:item.kind});
      run("berbr.navigate");
    }
    if (props()?.overlay) closeOverlay(props().overlay);
  }

  receiveJump(cx) {
    const target = shared("berbr.jumpTarget");
    if (!target || target.project !== this.project || !this.data) return;
    setShared("berbr.jumpTarget", null);
    const flat = walk(this.data.rows);
    const parent = flat.find(row => row.id === (target.task ?? target.parent ?? target.row));
    if (!parent) return;
    this.filter = ""; this.inputs.search.set_value(""); this.values.search = "";
    if (parent.done) this.all = true;
    this.setExpanded(parent.id, cx);
    for (const section of this.liveState().sections) if (section.rows.some(row => row.id === parent.id)) this.collapsed.delete(section.id);
    if (parent.kind === "task" && target.kind === "mr" && this.liveState().detail(parent).history.some(item => item.row.id === target.row))
      this.history.add(parent.id);
    const subtask = parent.kind === "task" && this.liveState().detail(parent).subtasks
      .find(row => walk([row]).some(child => child.id === target.row));
    if (subtask) this.subtasks.add(parent.id);
    const details = parent.kind === "task" ? this.detailItems(parent) : this.looseItems(parent);
    const item = details.find(item => item.row?.id === target.row)
      ?? details.find(item => item.type === "tree" && item.mr?.id === target.row)
      ?? details.find(item => item.row?.id === subtask?.id);
    this.selected = target.row === parent.id ? parent.id : item?.id ?? parent.id;
    this.handle.focus(); cx.notify();
  }

  toggleSection(id, cx, expanded = this.collapsed.has(id)) {
    if (this.busy) return;
    this.selected = `section-${id}`; this.handle.focus();
    if (expanded) this.collapsed.delete(id); else this.collapsed.add(id);
    try { localStorage.setItem("berbr.collapsed", JSON.stringify([...this.collapsed])); } catch (_) { /* Storage is optional. */ }
    cx.notify();
  }

  hoverRow(id, hovered, cx) {
    const next = hovered ? id : this.hovered === id ? null : this.hovered;
    if (next !== this.hovered) { this.hovered = next; cx.notify(); }
  }

  perform(cx, args, {close = false, action = args[0], before = null} = {}) {
    if (this.busy || this.loading) return;
    this.busy = true; this.busyAction = action; this.error = null;
    cx.notify();
    cx.spawn(async cx => {
      try {
        if (before) await before();
        const result = await this.cli(args);
        this.busy = false;
        run("berbr.refresh");
        if (close && props()?.overlay) closeOverlay(props().overlay);
        notify({title:"Berbr",body:result.trim() || "Done",level:"info"});
        if (!this.form) this.refresh(cx, true);
      } catch (error) { this.error = String(error.message ?? error); this.busy = false; }
      this.busyAction = null; cx.notify();
    });
  }

  openUrl(url, cx, action = url, {close = false} = {}) {
    if (!url || this.busy) return;
    this.busy = true; this.busyAction = action; this.error = null;
    cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await exec("open", [url]);
        if (result.code !== 0) throw new Error(result.stderr.trim() || "Could not open link");
        if (close && props()?.overlay) closeOverlay(props().overlay);
      } catch (error) { this.error = String(error.message ?? error); }
      this.busy = false; this.busyAction = null; cx.notify();
    });
  }

  openNote(row) {
    if (!row.note || this.busy) return;
    openOverlay({mod:"berbr", kind:"window", key:"berbr-note", width:720, height:0.75,
      props:{form:"note", path:row.note, title:row.title ?? row.label ?? row.task ?? "Note",
        tickets:walk(row.children).filter(child => child.kind === "ticket").map(ticket => ({key:ticket.target, url:ticket.url}))}});
  }

  open(row, cx, action = row?.id) {
    if (!row || this.busy) return;
    if (row.kind === "agent") {
      run("agent.focus", {terminal:row.terminal});
      if (props()?.overlay) closeOverlay(props().overlay);
    } else if (row.kind === "mr" || row.kind === "ticket") this.openUrl(row.url, cx, action);
    else if (row.kind === "task" || row.kind === "tree") {
      if (!row.task) {
        if (row.kind === "tree") this.removeTree(row, row.children?.find(child => child.kind === "mr"));
        else { this.error = "Task name is unavailable."; cx.notify(); }
        return;
      }
      const trees = walk(row.children).filter(item => item.kind === "tree");
      const args = ["select", row.task];
      const tree = row.tree ?? trees[0]?.tree;
      if (tree) args.push("--tree", tree);
      this.perform(cx, args, {close:true, action});
    }
  }

  formOverlay(form, extra = {}) {
    if (this.busy) return;
    const popup = ["projects", "attach-task", "menu"].includes(form);
    openOverlay({ mod:"berbr", kind: popup ? "popup" : "dialog",
      width: ["done", "remove-merged"].includes(form) ? 520 : ["projects", "menu"].includes(form) ? 220 : form === "attach-task" ? 300 : ["jump", "new"].includes(form) ? 560 : form === "remove-tree" ? 440 : 480,
      height: ["projects", "attach-task"].includes(form) ? 0.6 : popup ? undefined : form === "jump" ? 0.6 : ["done", "remove-merged"].includes(form) ? 0.75 : form === "new" ? 380 : form === "ticket" ? 420 : form === "remove-tree" ? 360 : 240,
      modal: ["new", "rename", "stop-agent", "remove-tree", "done", "remove-merged"].includes(form),
      ...(popup ? {at:"pointer"} : {}),
      props:{ form, project:this.project, binary:this.binary, agent:this.agent,
        ...this.actionContext(), ...extra } });
  }

  menuTarget(item) {
    if (item?.kind === "detail")
      return ["tree", "agent", "mr", "ticket", "subtask"].includes(item.type) ? item.row : null;
    return item;
  }

  openRowMenu(item, cx) {
    if (this.busy) return;
    const row = this.menuTarget(item);
    const mr = item?.mr ?? row?.children?.find(child => child.kind === "mr");
    const live = row?.terminal && this.liveState().live.get(row.terminal);
    if (!rowMenu(row, {spaces:spaces(), mr, live}).length) return;
    // Inline links may represent a row which also appears as a detail item.
    this.selected = this.rows().find(candidate => candidate.id === item.id || candidate.row?.id === row.id)?.id ?? item.id;
    this.contextRow = item;
    this.handle.focus(); cx.notify();
    this.formOverlay("menu", {row, mr, live, rows:this.data?.rows ?? []});
  }

  chooseMenu(cx, id = this.menuItems?.[this.menuIndex]?.id) {
    if (this.busy || !this.menuItems?.some(item => item.id === id)) return;
    const p = props(), row = p.row;
    const close = () => { if (p.overlay) closeOverlay(p.overlay); };
    const form = (name, extra) => { close(); this.formOverlay(name, extra); };
    const terminal = p.live?.address ?? row.terminal;
    switch (id) {
      case "space": {
        const space = taskSpace(row, spaces());
        if (space) { close(); run("space.select", {id:space.id}); }
        else { this.error = "Space is no longer available."; cx.notify(); }
        break;
      }
      case "start": case "resume": this.open(row, cx); break;
      case "review": this.perform(cx, ["review", row.tree], {close:true}); break;
      case "rename": form("rename", {row, prefill:{title:row.title ?? row.label ?? row.task}}); break;
      case "note": close(); this.openNote(row); break;
      case "done": form("done", {task:row.task, label:row.title ?? row.label, row}); break;
      case "remove-tree": close(); this.removeTree(row, p.mr, p.rows); break;
      case "stop-agent": form("stop-agent", {row, terminal}); break;
      case "agent": this.open({...row, terminal}, cx); break;
      case "url": this.openUrl(row.url, cx, id, {close:true}); break;
      case "mr": this.openUrl(p.mr?.url ?? row.children?.find(child => child.kind === "mr")?.url, cx, id, {close:true}); break;
      case "copy-title": this.copyText(row.title ?? row.label ?? row.task, cx); break;
      case "copy-path": this.copyText(row.cwd || treeMarks(row).path, cx); break;
      case "copy-branch": this.copyText(row.branch, cx); break;
      case "copy-terminal": this.copyText(terminal, cx); break;
      case "copy-link": this.copyText(row.url, cx); break;
    }
  }

  copyText(value, cx) {
    if (this.busy || value == null) return;
    this.busy = true; this.error = null; cx.notify();
    cx.spawn(async cx => {
      try {
        // Text is an argv entry; shell metacharacters and leading dashes stay literal.
        const result = await exec("/bin/sh", ["-c", 'printf %s "$1" | /usr/bin/pbcopy', "berbr-copy", String(value)]);
        if (result.code !== 0) throw new Error(result.stderr.trim() || "Could not copy text");
        notify({title:"Berbr", body:"Copied", level:"info"});
        if (props()?.overlay) closeOverlay(props().overlay);
      } catch (error) { this.error = String(error.message ?? error); }
      this.busy = false; cx.notify();
    });
  }

  submitRename(cx) {
    const title = this.values.title?.trim(), task = props()?.row?.task;
    if (!title || !task) { this.error = "Enter a task title."; cx.notify(); return; }
    this.perform(cx, ["rename", task, title], {close:true});
  }

  submitStopAgent(cx) {
    const terminal = props()?.terminal;
    if (this.busy || !terminal) return;
    this.busy = true; this.error = null; cx.notify();
    cx.spawn(async cx => {
      try {
        const result = await hc(["kill", terminal]);
        if (result.code !== 0) throw new Error(result.stderr.trim() || result.stdout.trim() || "Could not stop agent");
        run("berbr.refresh");
        notify({title:"Berbr", body:"Agent stopped", level:"info"});
        if (props()?.overlay) closeOverlay(props().overlay);
      } catch (error) { this.error = String(error.message ?? error); }
      this.busy = false; cx.notify();
    });
  }

  loadClosePlan(cx) {
    this.planLoading = true;
    cx.spawn(async cx => {
      try {
        const args = this.form === "remove-merged" ? ["done", "--merged"] : ["done", props().task];
        const plan = JSON.parse(await this.cli([...args, "--dry-run", "--json"]));
        if (plan?.kind !== (this.form === "remove-merged" ? "trees" : "task")) throw new Error("Invalid close plan kind");
        const accepted = backendClosePlan(plan);
        if (this.form === "remove-merged") this.bulkPlan = plan;
        else this.closePlan = accepted;
      } catch (error) {
        // Old binaries do not know dry-run. Keep the snapshot preview in that case.
        if (!/(?:unknown|unexpected|unrecognized|unexpectedly found|not supported|unsupported).*?(?:dry-run|--json)|(?:dry-run|--json).*?(?:unknown|unexpected|unrecognized|not supported|unsupported)/i.test(String(error.message ?? error)))
          this.error = String(error.message ?? error);
        this.planError = Boolean(this.error);
      }
      this.planLoading = false; cx.notify();
    });
  }

  submitNew(cx) {
    if (this.ticketMode === "existing" && !/^[A-Z][A-Z0-9]*-\d+$/.test(this.values.ticket?.trim() ?? "")) {
      this.error = "Enter a valid Jira ticket key (for example DEMO-123)."; cx.notify(); return;
    }
    const title = this.values.title?.trim();
    const args = ["start", ...(title ? [title] : ["--unnamed"]), "--bg"];
    if (this.agent === "none") args.push("--no-agent"); else args.push("--agent", this.agent);
    if (!this.worktree) args.push("--no-worktree");
    if (this.ticketMode === "new") args.push("--new-ticket");
    const ticket = this.values.ticket?.trim();
    if (this.ticketMode === "existing" && ticket) args.push("--ticket", ticket);
    const branch = this.values.branch?.trim();
    if (this.worktree && branch) args.push("--branch", branch);
    const name = this.values.name?.trim();
    if (this.worktree && name) args.push("--name", name);
    this.perform(cx, args, {close:true});
  }

  submitTicket(cx, worktree) {
    const ticket = this.values.ticket?.trim();
    if (!ticket) { this.error = "Enter a Jira ticket key."; cx.notify(); return; }
    const task = props()?.task;
    const args = worktree ? ["add", task, ticket] : ["attach", ticket, task];
    if (worktree) {
      const name = this.values.name?.trim(), existing = this.ticketTree;
      if (existing) args.push("--in", existing);
      else if (name) args.push("--name", name);
      else if (props()?.tickets?.includes(ticket)) {
        this.error = "Choose an existing worktree or enter a new worktree name."; cx.notify(); return;
      }
    }
    this.perform(cx, args, {close:true});
  }

  removeTree(tree, mr, rows = this.data?.rows ?? []) {
    this.formOverlay("remove-tree", {tree, mr, rows});
  }

  submitRemoveTree(cx) {
    const p = props() ?? {};
    const name = p.tree?.tree;
    if (!name?.trim()) { this.error = "Worktree name is unavailable."; cx.notify(); return; }
    const target = p.tree?.cwd || name;
    const args = ["done", "--tree", target, "--yes"];
    if (p.tree?.repo) args.push("--repo", p.tree.repo);
    this.perform(cx, args, {close:true, action:"remove-tree"});
  }

  enterTree(tree, mr, cx, action, remove = false) {
    if (this.busy) return;
    if (remove || !tree.task) this.removeTree(tree, mr);
    else this.open(tree, cx, action);
  }

  navigateTree(tree, cx, id = tree.id) {
    if (this.busy) return;
    this.selected = id; this.handle.focus();
    const target = treeFocusTarget(tree, agents(), panes());
    if (target?.agent) run("agent.focus", {terminal:target.terminal});
    else if (target) {
      if (target.tab) run("tab.select", {id:target.tab});
      run("pane.focus", {terminal:target.terminal});
    }
    cx.notify();
  }

  setExpanded(id, cx) {
    this.expanded = id;
    try { localStorage.setItem("berbr.expanded", JSON.stringify(id)); } catch (_) { /* Storage is optional. */ }
    cx.notify();
  }

  taskClick(row, cx) {
    if (this.busy) return;
    this.selected = row.id; this.handle.focus();
    if (row.kind === "waiting") { cx.notify(); return; }
    if (row.kind === "section") this.toggleSection(row.section, cx);
    else if (row.kind === "detail") row.action?.(cx);
    else if (row.kind === "tree") this.navigateTree(row, cx);
    else this.setExpanded(this.expanded === row.id ? null : row.id, cx);
    cx.notify();
  }

  showDetails(row, cx) {
    if (this.busy) return;
    if (row.kind === "section") { this.toggleSection(row.section, cx, true); return; }
    const task = row.kind === "detail" ? row.parent : row;
    this.setExpanded(task.id, cx);
  }

  enter(row, cx) {
    if (this.busy) return;
    if (row.kind === "section") this.toggleSection(row.section, cx);
    else if (row.kind === "task") {
      const space = taskSpace(row, spaces());
      if (space) run("space.select", {id:space.id});
      else this.setExpanded(row.id, cx);
    } else this.taskClick(row, cx);
  }

  key(event, cx) { return this.withLive(() => this.keyEvent(event, cx)); }

  keyEvent(event, cx) {
    if (this.form === "note" && noteKey(this, event, cx)) return;
    if (["projects", "attach-task"].includes(this.form) && ["up", "down", "enter"].includes(event.key)) {
      if (event.key === "enter") this.choosePicker(cx);
      else if (!this.busy) this.pickerIndex = Math.max(0, Math.min(this.pickerMatches().length - 1,
        this.pickerIndex + (event.key === "down" ? 1 : -1)));
      cx.stop_propagation(); cx.notify(); return;
    }
    if (this.form === "menu" && ["up", "down", "enter"].includes(event.key)) {
      if (event.key === "enter") this.chooseMenu(cx);
      else if (!this.busy && this.menuItems.length)
        this.menuIndex = (this.menuIndex + (event.key === "down" ? 1 : -1) + this.menuItems.length) % this.menuItems.length;
      cx.stop_propagation(); cx.notify(); return;
    }
    if (!this.form && this.panelFocused() && !this.searchFocused && !this.replyFocused &&
        (event.keystroke === "shift-f10" || ["contextmenu", "menu", "apps"].includes(event.key))) {
      this.openRowMenu(this.chosen(), cx); cx.stop_propagation(); return;
    }
    if (!this.form && this.panelFocused() && event.keystroke === "cmd-k") {
      this.openJump(); cx.stop_propagation(); return;
    }
    if (this.form === "jump" && ["up", "down", "enter"].includes(event.key)) {
      if (event.key === "enter") this.chooseJump(cx);
      else this.jumpIndex = Math.max(0, Math.min(this.jumpMatches().length - 1, this.jumpIndex + (event.key === "down" ? 1 : -1)));
      cx.stop_propagation(); cx.notify(); return;
    }
    if (this.replyFocused) {
      if (event.key === "escape") { this.replyFocused = null; this.handle.focus(); cx.stop_propagation(); }
      return;
    }
    if (event.key === "escape") {
      if (!this.form && this.searchFocused) {
        this.filter = ""; this.values.search = ""; this.inputs.search.set_value("");
        this.handle.focus(); cx.stop_propagation(); cx.notify(); return;
      }
      if (props()?.overlay) {
        if (!this.busy || this.form === "note") closeOverlay(props().overlay);
        cx.stop_propagation(); return;
      }
    }
    if (this.form || !this.panelFocused() || this.busy || event.modifiers?.control || event.modifiers?.platform || event.modifiers?.alt) return;
    const arrow = ["up", "down"].includes(event.key);
    if (this.searchFocused && !arrow) return;
    const rows = this.rows();
    const index = rows.findIndex(row => row.id === this.selected);
    if (arrow || ["j", "k"].includes(event.key)) {
      const delta = ["down", "j"].includes(event.key) ? 1 : -1;
      this.selected = rows[Math.max(0, Math.min(rows.length - 1, index + delta))]?.id ?? null;
    } else if (/^[1-9]$/.test(event.key) && this.chosen()?.kind === "waiting") this.answerAgent(this.chosen(), event.key, cx);
    else if (event.key === "enter" && this.chosen()?.kind === "waiting") this.replyTo(this.chosen(), cx);
    else if (event.key === "enter" && this.chosen()) this.enter(this.chosen(), cx);
    else if (event.key === "left" && this.chosen()) {
      const row = this.chosen();
      if (row.kind === "section") this.toggleSection(row.section, cx, false);
      else if (row.kind === "detail") this.selected = row.parent.id;
      else if (row.id === this.expanded) this.setExpanded(null, cx);
    } else if (["right", "space", " "].includes(event.key) && this.chosen()) this.showDetails(this.chosen(), cx);
    else if (event.key === "/") this.inputs.search.focus();
    else if (event.key === "n") this.formOverlay("new");
    else if (event.key === "r") this.refresh(cx, false, true);
    else if (event.key === "d") this.all = !this.all;
    else return;
    cx.stop_propagation(); cx.notify();
  }

  input(key) {
    const t = theme();
    return Input.new(this.inputs[key]).h(32).w_full().px(10).rounded(6)
      .border_1().border_color(t.border).bg(t.background).text_color(t.foreground).text_size(13);
  }

  panelFocused() { return this.handle.is_focused() || this.searchFocused; }

  currentTree(cwd) {
    if (this.focusTreeData !== this.data || this.focusTreeCwd !== cwd) {
      this.focusTreeData = this.data; this.focusTreeCwd = cwd;
      this.focusTree = focusedTree(this.data?.rows, cwd);
    }
    return this.focusTree;
  }

  taskRow(t, row) {
    const signal = this.signal(row);
    const selected = this.panelFocused() && row.id === this.selected;
    const active = selected || signal.liveSpace?.active;
    const hints = h_flex().overflow_hidden().gap(6).items_center().min_w(0).text_size(11);
    const showMr = signal.mrNeedsYou || signal.mrFlag === "ready to merge";
    if (signal.working && !showMr) {
      const activity = this.activity(signal.workingAgent, signal.workingTree?.tree ?? signal.workingTree?.label);
      hints.child(h_flex().min_w(0).gap(4).items_center().text_color(t.muted)
        .child(div().min_w(0).truncate().child(activity.text))
        .when(Boolean(activity.elapsed), el => el.child(div().flex_none().text_color(t.dim).child(activity.elapsed)))
        .when(signal.working > 1, el => el.child(div().flex_none().child(`+${signal.working - 1}`))));
    } else if (["blocked", "done"].includes(signal.status) && signal.agents.some(agent => agent.status === signal.status)) hints.child(div().min_w(0).truncate()
      .text_color(t[signal.status]).child(this.attention(signal.agents.filter(agent => agent.status === signal.status)
        .reduce((latest, agent) => !latest || (agent.seq ?? 0) > (latest.seq ?? 0) ? agent : latest, null))));
    if (!signal.working && !signal.mrNeedsYou && signal.ticket) hints.child(div().min_w(0).truncate().text_color(t.muted)
      .child(`${signal.ticket.target}${normalizeTicketState(signal.ticket.state) === "In review" ? " in review" : ""}`));
    if ((!signal.working || showMr) && signal.mrs.length) {
      const mr = signal.mrs[0], spec = mrPills(mr)[0];
      hints.child(this.externalLink(t, `task-mr-${row.id}`, mr, [
        div().flex_none().child(signal.mrKey),
        spec && pill(t, {...spec, tooltip:`${spec.label} — open ${signal.mrKey} in GitLab`}),
        signal.mrs.length > 1 && div().flex_none().text_color(t.dim).child(`+${signal.mrs.length - 1}`),
      ]).min_w(0));
    }
    const hasSignal = Boolean(signal.attention || signal.ticket || signal.mrs.length);
    if (!hasSignal && signal.trees.length) hints.child(div().text_color(t.dim)
      .child(`${signal.trees.length} ${signal.trees.length === 1 ? "worktree" : "worktrees"}`));
    const expanded = row.id === this.expanded;
    const secondLine = !expanded && (hasSignal || signal.trees.length > 0);
    return h_flex().id(`task-${row.id}`).min_h(secondLine ? 44 : 32).px(8).py(6).gap(10)
      .items_start().rounded(6).when(active, el => el.bg(t.selected)).cursor_pointer()
      .opacity(this.busy ? 0.4 : 1)
      .hover(el => el.bg(active ? t.selected : t.hover))
      .on_hover((hovered, cx) => this.hoverRow(row.id, hovered, cx))
      .on_click((_event, cx) => this.taskClick(row, cx))
      .on_mouse_down("right", (_event, cx) => {
        this.openRowMenu(row, cx); cx.stop_propagation();
      })
      .child(div().pt(2).child(statusMark(t, signal.status)))
      .child(v_flex().flex_1().min_w(0).gap(2)
        .child(h_flex().gap(6).items_center().min_w(0)
          .child(div().flex_1().min_w(0).text_size(13).truncate().child(row.title ?? row.label ?? row.task ?? "Untitled task"))
          .when(Boolean(signal.liveSpace), el => el.child(iconButton(t, `task-space-${row.id}`, "monitor", "Go to space",
            () => run("space.select", {id:signal.liveSpace.id}), {size:20, disabled:this.busy}))))
        .when(secondLine, el => el.child(hints)))
      .child(div().w(22).flex_none().when(!(expanded || this.hovered === row.id || selected), el => el.invisible()).child(iconButton(t, `details-${row.id}`,
        expanded ? "chevron-down" : "chevron-right", expanded ? "Collapse task" : "Expand task",
        cx => this.taskClick(row, cx), {size:22, disabled:this.busy})));
  }

  detailItems(task, detail = this.liveState().detail(task)) {
    const items = [];
    const add = (type, key, values, action) => {
      const id = `${task.id}/${type}/${key}`;
      items.push({id, kind:"detail", parent:task, type, ...values,
        action:action ? cx => action(cx, id) : null});
    };
    for (const [index, tree] of detail.trees.entries()) {
      add("tree", tree.row.id, {...tree, groupStart:index > 0, hasAgents:tree.agents.length > 0}, (cx, id) => this.navigateTree(tree.row, cx, id));
      for (const agent of tree.agents) add("agent", `${tree.row.id}/${agent.row.id}`, {...agent, tree:tree.row, nested:true},
        cx => this.open(agent.row, cx));
    }
    if (detail.mainAgents.length) {
      add("main", "header", {label:"Main checkout", groupStart:detail.trees.length > 0, hasAgents:true});
      for (const agent of detail.mainAgents) add("agent", agent.row.id, {...agent, nested:true}, cx => this.open(agent.row, cx));
    }
    for (const mr of detail.openMrs) add("mr", mr.row.id, mr, (cx, id) => this.openUrl(mr.row.url, cx, id));
    for (const [key, label, rows, expanded] of [
      ["history", `History · ${detail.history.length} MRs`, detail.history, this.history],
      ["subtasks", `Subtasks · ${detail.subtasks.length}`, detail.subtasks, this.subtasks],
    ]) {
      if (!rows.length) continue;
      add("disclosure", key, {label, expanded:expanded.has(task.id)}, cx => {
        if (expanded.has(task.id)) expanded.delete(task.id); else expanded.add(task.id);
        cx.notify();
      });
      if (expanded.has(task.id)) for (const entry of rows) {
        const row = key === "history" ? entry.row : entry;
        add(key === "history" ? "mr" : "subtask", row.id,
          key === "history" ? {...entry, historical:true} : {row},
          (cx, id) => key === "history" ? this.openUrl(row.url, cx, id) : this.openNote(row));
      }
    }
    if (!detail.trees.length && !taskSpace(task, this.liveState().spaces))
      add("action", "start", {label:"Start work", accent:true}, (cx, id) => this.open(task, cx, id));
    if (task.note) add("action", "note", {label:"Note"}, () => this.openNote(task));
    if (!task.done) add("action", "close", {label:"Close task", close:true}, () => this.formOverlay("done", {
      task:task.task, label:task.title ?? task.label, row:task, plan:this.liveState().plan(task),
    }));
    return items;
  }

  looseItems(row) {
    const items = [];
    const add = (type, key, values, action) => {
      const id = `${row.id}/loose/${type}/${key}`;
      items.push({id, kind:"detail", parent:row, type, ...values, action:cx => action(cx, id)});
    };
    const ticket = row.kind === "ticket" ? row : null;
    if (ticket) add("ticket", row.id, {row}, (cx, id) => this.openUrl(row.url, cx, id));
    const trees = row.kind === "tree" ? [row] : (row.children ?? []).filter(child => child.kind === "tree");
    for (const tree of trees) {
      const mr = tree.children?.find(child => child.kind === "mr");
      add("tree", tree.id, {row:tree, mr, loose:true, ticket}, (cx, id) => this.navigateTree(tree, cx, id));
    }
    const mrs = new Map((row.kind === "mr" ? [row] : walk(row.children)).filter(child => child.kind === "mr")
      .map(mr => [mr.url || mr.id || mr, mr]));
    for (const mr of mrs.values()) add("mr", mr.id, {row:mr, tickets:[], historical:["merged", "closed"].includes(mr.state?.toLowerCase())},
      (cx, id) => this.openUrl(mr.url, cx, id));
    if (ticket && !isClosedTicket(ticket)) add("action", "make-task", {label:"Make task"}, () => this.makeTask(ticket));
    if (ticket) add("action", "attach", {label:"Attach to task…"}, () => {
      const tasks = [...new Map(walk(this.data?.rows).filter(task => task.kind === "task" && !task.done && task.task)
        .map(task => [task.task, task])).values()];
      this.formOverlay("attach-task", {ticket:ticket.target, tasks});
    });
    if (["ticket", "mr"].includes(row.kind)) add("action", "url", {
      label:ticket ? "Open in Jira" : "Open in GitLab", link:true,
    }, (cx, id) => this.openUrl(row.url, cx, id));
    return items;
  }

  makeTask(ticket) {
    this.formOverlay("new", {prefill:{title:ticketTitle(ticket), ticket:ticket.target ?? ""}});
  }

  treeState(t, row, {compact = false, hideMerged = false, hideChanges = false} = {}) {
    const marks = treeMarks(row);
    const parts = [];
    if (marks.changes && !hideChanges) parts.push([`${marks.changes} uncommitted`, t.working,
      `${marks.changes} uncommitted ${marks.changes === 1 ? "file" : "files"}`]);
    if (marks.unpushed) parts.push([`↑${marks.unpushed}${compact ? "" : " unpushed"}`, t.blocked,
      `${marks.unpushed} unpushed ${marks.unpushed === 1 ? "commit" : "commits"}`]);
    if (marks.merged && !hideMerged) parts.push(["merged", t.dim, "merged"]);
    if (marks.quietDays !== null) parts.push([`${compact ? "" : "quiet "}${marks.quietDays}d`, t.dim, `quiet ${marks.quietDays}d`]);
    if (!parts.length) return null;
    return h_flex().min_w(0).gap(6).text_size(11).items_center().tooltip(parts.map(([, , label]) => label).join(" · "))
      .when(compact, el => el.flex_none())
      .children(parts.map(([label, tone]) => div().when(compact, el => el.flex_none())
        .when(!compact, el => el.min_w(0).truncate()).text_color(tone).child(label)));
  }

  innerRow(t, item) {
    const selected = this.panelFocused() && item.id === this.selected;
    const pending = this.busy && this.busyAction === item.id;
    const focus = focused();
    const here = item.type === "agent" ? Boolean(focus?.terminal &&
      [item.row?.terminal, item.live?.address, item.live?.pane].includes(focus.terminal))
      : item.type === "tree" && this.currentTree(focus?.cwd) === item.row;
    return h_flex().relative().id(item.id).h(item.type === "agent" ? 24 : 28).flex_none().px(6).gap(6).items_center().rounded(6)
      .min_w(0).text_size(12.5).text_color(t.foreground)
      .when(selected, el => el.bg(t.selected)).hover(el => el.bg(selected ? t.selected : t.hover))
      .when(here, el => el.child(div().absolute().left(0).top(4).bottom(4).w(2).bg(t.accent)
        .tooltip("You are here").accessibility_label("You are here")))
      .opacity(this.busy && !pending && ![`${item.id}/resume`, `${item.id}/mr`, `${item.id}/ticket`].includes(this.busyAction) ? 0.4 : 1)
      .when(Boolean(item.action), el => el.cursor_pointer()
        .on_hover((hovered, cx) => this.hoverRow(item.id, hovered, cx))
        .on_click((_event, cx) => { cx.stop_propagation(); this.taskClick(item, cx); }))
      .when(Boolean(this.menuTarget(item)), el => el.on_mouse_down("right", (_event, cx) => {
        this.openRowMenu(item, cx); cx.stop_propagation();
      }));
  }

  treeActions(t, tree, mr, id, {remove = false, suffix = "", loose = false, ticket = null} = {}) {
    const primaryRemove = !tree.task;
    const closed = remove || isMergedTree(tree) || [tree.mr_state, mr?.state]
      .some(state => ["merged", "closed"].includes(state?.toLowerCase()));
    const action = `${id}/${primaryRemove ? "remove" : "resume"}${suffix ? `/${suffix}` : ""}`;
    const shown = this.hovered === id || (this.panelFocused() && this.selected === id) ||
      [action, `${id}/resume`, `${id}/remove`].includes(this.busyAction);
    const group = buttons => [h_flex().flex_none().gap(6).items_center()
      .when(!shown, el => el.invisible()).children(buttons)];
    if (loose && !tree.task) {
      const buttons = [iconButton(t, `${id}/remove`, "archive", "Remove worktree", () => this.removeTree(tree, mr),
        {size:20, disabled:this.busy})];
      if (ticket && !isClosedTicket(ticket) && ticket.target) buttons.push(iconButton(t, `${id}/resume`, "terminal", "Resume agent",
        cx => this.perform(cx, ["select", ticket.target], {close:true, action:`${id}/resume`}),
        {size:20, disabled:this.busy, spinning:this.busy && this.busyAction === `${id}/resume`}));
      return group(buttons);
    }
    const buttons = [iconButton(t, action, primaryRemove ? "archive" : "terminal", primaryRemove ? "Remove worktree" : "Resume agent",
      cx => this.enterTree(tree, mr, cx, action),
      {size:20, disabled:this.busy, spinning:this.busy && this.busyAction === action})];
    if (tree.task && closed) buttons.push(iconButton(t, `${id}/remove`, "archive", "Remove worktree",
      () => this.removeTree(tree, mr), {size:20, disabled:this.busy}));
    return group(buttons);
  }

  externalLink(t, id, row, content = []) {
    const mr = row.kind === "mr";
    const key = mr ? row.label?.match(/!\d+/)?.[0] ?? "MR" : row.target ?? "ticket";
    return h_flex().id(id).min_w(0).gap(4).items_center().text_color(t.muted).cursor_pointer()
      .tooltip(`Open ${key} in ${mr ? "GitLab" : "Jira"}`)
      .opacity(this.busy && this.busyAction !== id ? 0.4 : 1)
      .hover(el => el.text_color(t.foreground).underline())
      .when(this.panelFocused() && this.selected === row.id, el => el.bg(t.selected))
      .on_mouse_down("right", (_event, cx) => {cx.stop_propagation(); this.openRowMenu(row, cx);})
      .on_click((_event, cx) => {cx.stop_propagation(); this.openUrl(row.url, cx, id);})
      // The spinner slot trails the content: reserved so nothing shifts, never a leading gap.
      .children(content.filter(Boolean))
      .child(div().size(11).flex_none().when(!(this.busy && this.busyAction === id), el => el.invisible())
        .child(icon("spinner", 11).text_color(t.dim).when(this.busy && this.busyAction === id, el => el.spin(900))));
  }

  mrState(t, mr, historical = false, state = mr?.state?.toLowerCase()) {
    const key = mr?.label?.match(/!\d+/)?.[0] ?? "MR";
    return h_flex().flex_none().gap(4).items_center().text_color(historical || ["merged", "closed"].includes(state) ? t.dim : t.muted)
      .children(mrPills(mr, {historical, state}).map(spec => pill(t, {...spec,
        tooltip:`${spec.label} — open ${key} in GitLab`})));
  }

  agentLine(t, line, item) {
    const status = item.live && ["working", "blocked", "done", "idle"].includes(item.status) ? item.status : "unknown";
    const treeName = item.tree?.tree ?? item.tree?.label;
    const activity = status === "working" ? this.activity(item.live, treeName) : null;
    const title = activity ? activity.text : agentActivity(item.live?.title?.trim() || item.row.label || "Agent", null, treeName);
    const program = item.live?.program;
    const mark = program?.icon
      ? program.tint ? svg(program.icon).size(14).text_color(t.muted) : image(program.icon).size(14)
      : icon("terminal", 14).text_color(t.dim);
    const label = activity ? activity.elapsed : !item.live ? "stopped"
      : status === "blocked" ? this.attention(item.live) : status === "done" ? "finished" : "";
    return line.text_size(12).text_color(item.live ? t.muted : t.dim)
      .child(mark.flex_none().tooltip(program?.name || "Terminal").accessibility_label(program?.name || "Terminal"))
      .child(div().flex_1().min_w(0).truncate().tooltip(title).when(Boolean(activity), el => el.text_color(t.muted)).child(title))
      .child(h_flex().min_w(0).max_w("50%").gap(4).items_center().text_size(11)
        .text_color(["blocked", "done"].includes(status) ? t[status] : t.dim)
        .child(activity ? icon("spinner", 11).text_color(t.working).spin(900) : statusMark(t, status))
        .when(Boolean(label), el => el.child(div().min_w(0).truncate().tooltip(label).child(label))));
  }

  detailRow(t, item) {
    const pending = this.busy && this.busyAction === item.id;
    const line = this.innerRow(t, item);
    const mark = name => pending ? icon("spinner", 11).text_color(t.dim).spin(900) : icon(name, 12).text_color(t.dim);
    if (item.type === "tree") {
      if (!treeFocusTarget(item.row, agents(), panes())) line.tooltip("No agent here — use Resume");
      line.child(mark("git-branch"))
        .child(div().flex_1().min_w(0).truncate()
          .text_color(isMergedTree(item.row) || ["merged", "closed"].includes(item.mr?.state?.toLowerCase()) ? t.muted : t.foreground)
          .tooltip(item.row.label ?? item.row.tree ?? "Untitled").child(item.row.label ?? item.row.tree ?? "Untitled"));
      const summary = [item.row.branch, item.row.body, item.mr?.label, ...mrPills(item.mr).map(spec => spec.label),
        item.row.diff ? `+${item.row.diff.adds} −${item.row.diff.dels}` : ""].filter(Boolean).join(" · ");
      const right = h_flex().min_w(0).max_w("35%").gap(6).items_center().overflow_hidden().tooltip(summary);
      if (item.loose && item.row.branch) right.child(div().flex_1().min_w(0).truncate()
        .text_size(11).text_color(t.dim).tooltip(item.row.branch).child(item.row.branch));
      const state = this.treeState(t, item.row, {compact:true, hideMerged:Boolean(item.mr && !item.loose), hideChanges:!item.loose});
      if (state) right.child(state.min_w(0).overflow_hidden());
      if (item.mr && !item.loose) right.child(this.externalLink(t, `${item.id}/mr`, item.mr, [
        div().flex_none().text_size(12).child(item.mr.label?.match(/!\d+/)?.[0] ?? "MR"),
        this.mrState(t, item.mr, false, isMergedTree(item.row) ? "merged"
          : item.row.mr_state ?? item.mr.state?.toLowerCase()),
      ]).min_w(0).overflow_hidden());
      else if (!item.loose) right.child(div().flex_none().text_size(11).text_color(t.dim).child("no MR"));
      const changes = treeMarks(item.row).changes;
      if (!item.loose && changes) right.child(div().flex_none().size(6).rounded(3).bg(t.working)
        .tooltip(`${changes} uncommitted ${changes === 1 ? "file" : "files"}`)
        .accessibility_label(`${changes} uncommitted ${changes === 1 ? "file" : "files"}`));
      const diff = item.row.diff;
      if (diff && (diff.adds > 0 || diff.dels > 0)) right.child(h_flex().flex_none().gap(4).text_size(11)
        .child(div().flex_none().text_color(t.done ?? t.dim).child(`+${diff.adds}`))
        .child(div().flex_none().text_color(t.blocked ?? t.dim).child(`−${diff.dels}`)));
      line.child(right).children(this.treeActions(t, item.row, item.mr, item.id, {loose:item.loose, ticket:item.ticket}));
    } else if (item.type === "ticket") {
      line.child(this.externalLink(t, `${item.id}/ticket`, item.row, [div().child(item.row.target ?? "Ticket")])
        .flex_none().tooltip("Open in Jira"))
        .child(chip(t, normalizeTicketState(item.row.state)))
        .child(div().flex_1().min_w(0).truncate().child(ticketTitle(item.row)));
    } else if (item.type === "agent") {
      this.agentLine(t, line, item);
    } else if (item.type === "mr") {
      const iid = item.row.label?.match(/!\d+/)?.[0] ?? "MR";
      const title = item.row.label?.replace(/^\s*!\d+\s*/, "") ?? "";
      line.text_color(item.historical ? t.dim : t.foreground).child(mark("git-compare"))
        .child(this.externalLink(t, `${item.id}/mr`, item.row, [
          div().flex_none().child(iid),
          div().flex_1().min_w(0).truncate().child(title),
          this.mrState(t, item.row, item.historical),
        ]).flex_1().text_color(item.historical ? t.dim : t.foreground))
        .when(Boolean(item.tickets.length), el => el.child(div().max_w("35%").min_w(0).truncate()
          .text_size(11).text_color(t.dim).child(item.tickets.join(" · "))));
    } else if (item.type === "subtask") {
      line.child(pending ? mark("file-text") : statusMark(t, this.signal(item.row).status))
        .child(div().flex_1().min_w(0).truncate().child(item.row.title ?? item.row.label ?? "Untitled task"));
    } else if (item.type === "disclosure") {
      line.text_color(t.dim).child(mark(item.expanded ? "chevron-down" : "chevron-right"))
        .child(div().min_w(0).truncate().text_size(12).child(item.label));
    } else if (item.type === "main") {
      line.child(icon("folder", 12).text_color(t.dim))
        .child(div().text_size(12).text_color(t.muted).child(item.label));
    }
    if (item.nested) return div().pl(12).border_l_1().border_color(t.border).ml(12).child(line);
    return div().when(Boolean(item.groupStart), el => el.mt(4))
      .when(Boolean(item.hasAgents), el => el.relative().child(div().absolute().left(12).bottom(0).h(8).w(1).bg(t.border)))
      .child(line);
  }

  ticketsLine(t, task, detail) {
    const line = h_flex().min_h(26).gap(8).items_center().flex_wrap().min_w(0);
    const ticket = (row, closed = false) => this.externalLink(t, `${task.id}/${row.id}`, row, [
      div().text_size(12).child(row.target ?? row.label ?? "Ticket"),
      div().min_w(0).truncate().text_size(11).text_color(!closed && normalizeTicketState(row.state) === "In review" ? t.accent : t.dim)
        .child(normalizeTicketState(row.state)),
    ]).h(26).max_w("100%").text_color(closed ? t.dim : t.muted);
    line.children(detail.tickets.open.map(row => ticket(row)));
    if (!detail.tickets.open.length && !detail.tickets.closed.length)
      line.child(div().text_size(12).text_color(t.dim).child("No ticket"));
    if (detail.tickets.closed.length) {
      line.child(button(t, `${task.id}/closed-tickets`, `+${detail.tickets.closed.length} closed`, cx => {
        if (this.closedTickets.has(task.id)) this.closedTickets.delete(task.id); else this.closedTickets.add(task.id);
        cx.notify();
      }, "ghost", this.busy).h(26).px(0).text_color(t.dim));
      if (this.closedTickets.has(task.id)) line.children(detail.tickets.closed.map(row => ticket(row, true)));
    }
    return line.child(iconButton(t, `${task.id}/add-ticket`, "plus", "Add ticket", () => this.formOverlay("ticket", {
      task:task.task, title:task.title ?? task.label,
      tickets:[...detail.tickets.open, ...detail.tickets.closed].map(ticket => ticket.target),
      trees:detail.trees.map(tree => tree.row),
    }), {size:20, disabled:this.busy}));
  }

  taskBlock(t, task) {
    const waiting = this.waitingItems(task);
    if (task.id !== this.expanded) {
      if (!waiting.length) return this.taskRow(t, task);
      const rows = this.liveState().detail(task);
      return v_flex().child(this.taskRow(t, task)).children(waiting.map(item => {
        const agent = rows.trees.flatMap(tree => tree.agents.map(agent => ({...agent, tree:tree.row})))
          .concat(rows.mainAgents).find(agent => agent.live?.address === item.agent.address);
        return v_flex().child(this.agentLine(t, h_flex().h(24).mx(28).gap(6).items_center().min_w(0)
          .on_mouse_down("right", (_event, cx) => {
            cx.stop_propagation();
            this.openRowMenu({id:item.id, kind:"detail", type:"agent",
              row:agent?.row ?? {kind:"agent", terminal:item.agent.address}}, cx);
          }), agent ?? {
          row:{label:item.agent.title}, live:item.agent, status:"blocked",
        })).child(this.waitingCard(t, item));
      }));
    }
    const detail = this.liveState().detail(task);
    const items = this.detailItems(task, detail);
    const body = v_flex().pl(28).pr(8).pb(4).min_w(0);
    if (detail.excerpt) body.child(div().pb(4).child(div().px(6).max_h(32).overflow_hidden().line_height(1.35)
      .whitespace_normal().text_size(12).text_color(t.muted).child(detail.excerpt)));
    body.child(div().px(6).child(this.ticketsLine(t, task, detail)))
      .children(items.filter(item => item.type !== "action").map(item => v_flex().child(this.detailRow(t, item))
        .when(item.type === "agent" && item.live?.status === "blocked", el => {
          const prompt = waiting.find(prompt => prompt.agent.address === item.live.address);
          if (prompt) el.child(div().ml(12).border_l_1().border_color(t.border).child(this.waitingCard(t, prompt)));
        })));
    body.child(this.actionRows(t, items));
    body.children(waiting.filter(prompt => !items.some(item => item.type === "agent" && item.live?.address === prompt.agent.address))
      .map(prompt => this.waitingCard(t, prompt)));
    return this.expandedBlock(t, this.taskRow(t, task), body);
  }

  actionRows(t, items) {
    const actions = h_flex().min_h(28).flex_none().gap(2).items_center().min_w(0).flex_wrap();
    for (const item of items.filter(item => item.type === "action")) {
      if (item.close) actions.child(div().flex_1());
      const pending = this.busy && this.busyAction === item.id;
      actions.child(this.innerRow(t, item).text_size(12).text_color(item.accent ? t.accent : t.muted)
        .hover(el => el.bg(t.hover).text_color(item.close ? t.blocked : t.foreground))
        .when(item.link, el => el.tooltip(item.label).hover(el => el.text_color(t.foreground).underline()))
        .child(item.label).child(div().size(11).flex_none().when(!pending, el => el.invisible())
          .child(icon("spinner", 11).text_color(t.dim).when(pending, el => el.spin(900)))));
    }
    return actions;
  }

  expandedBlock(t, header, body) {
    return v_flex().my(4).flex_none().min_w(0).rounded(8).border_1().border_color(t.border).bg(t.background)
      .child(header).child(body);
  }

  looseContent(t, row, reason) {
    const tree = row.kind === "tree" ? row : row.children?.find(child => child.kind === "tree");
    const state = tree && this.treeState(t, tree);
    return v_flex().flex_1().min_w(0).gap(2)
      .child(div().min_w(0).truncate().text_size(12.5).child(row.label ?? row.target ?? "Untitled"))
      .child(h_flex().min_w(0).gap(4).items_center().text_size(11).text_color(t.dim).tooltip(reason)
        .child(div().min_w(0).truncate().child(reason))
        .when(Boolean(state), el => el.child("·").child(state)));
  }

  looseRow(t, row, section) {
    const selected = this.panelFocused() && row.id === this.selected;
    const expanded = row.id === this.expanded;
    const reason = section === "todo" ? "Assigned in Jira" : this.liveState().reason(row);
    const kindIcon = { ticket:"file-text", tree:"git-branch", mr:"git-compare" }[row.kind] ?? "file-text";
    const line = h_flex().id(`loose-${row.id}`).h(44).flex_none().px(8).gap(8).items_center().rounded(6)
      .when(selected, el => el.bg(t.selected)).hover(el => el.bg(selected ? t.selected : t.hover)).cursor_pointer()
      .opacity(this.busy ? 0.4 : 1)
      .on_hover((hovered, cx) => this.hoverRow(row.id, hovered, cx))
      .on_mouse_down("right", (_event, cx) => {cx.stop_propagation(); this.openRowMenu(row, cx);})
      .on_click((_event, cx) => this.taskClick(row, cx));
    if (row.kind === "agent") {
      const live = row.terminal && this.liveState().live.get(row.terminal);
      return this.agentLine(t, line, {row, live, status:live?.status});
    }
    return line.child(icon(kindIcon).text_color(t.dim))
      .child(this.looseContent(t, row, reason))
      .when(section === "todo" && row.kind === "ticket" && !isClosedTicket(row),
        el => el.child(div().flex_none().when(this.hovered !== row.id, el => el.invisible())
          .child(button(t, `make-task-${row.id}`, "Make task", () => this.makeTask(row), "ghost", this.busy))))
      .child(div().w(22).flex_none().when(!(expanded || this.hovered === row.id || selected), el => el.invisible()).child(iconButton(t, `details-${row.id}`,
        expanded ? "chevron-down" : "chevron-right", expanded ? "Collapse details" : "Expand details",
        cx => {
          this.selected = row.id; this.handle.focus();
          this.setExpanded(expanded ? null : row.id, cx);
        }, {size:22, disabled:this.busy})));
  }

  looseBlock(t, row, section) {
    const header = this.looseRow(t, row, section);
    if (row.id !== this.expanded) return header;
    const items = this.looseItems(row);
    const body = v_flex().pl(24).pr(8).pb(4).min_w(0)
      .child(div().px(6).py(4).text_size(12).text_color(t.muted).whitespace_normal().child(this.liveState().why(row)))
      .children(items.filter(item => item.type !== "action").map(item => this.detailRow(t, item)))
      .child(this.actionRows(t, items));
    return this.expandedBlock(t, header, body);
  }

  banner(t, id, message, dismiss) {
    const text = String(message).replace(/&/g, "&amp;").replace(/</g, "&lt;")
      .replace(/>/g, "&gt;").replace(/\n/g, "<br>");
    return h_flex().mx(10).px(10).py(8).gap(6).items_start().rounded(6).bg(t.raised)
      .border_l(2).border_color(t.blocked)
      .child(TextView.html(`banner-${id}`, text).flex_1().min_w(0).text_size(12).text_color(t.muted).selectable())
      .when(Boolean(dismiss), el => el.child(iconButton(t, id, "x", "Dismiss error", dismiss, {size:22})));
  }

  render(cx) {
    if (this.form) return this.renderPanel(cx);
    const desired = shared("berbr.project");
    if (desired && desired !== this.project && desired !== this.pendingProject && !this.loading && !this.busy) {
      this.pendingProject = desired;
      this.switchProject(desired, cx);
    } else if (desired === this.project && !this.loading) this.pendingProject = null;
    return this.withLive(() => this.renderPanel(cx));
  }

  renderPanel(cx) {
    const t = theme();
    if (this.form) return v_flex().size_full().min_h(0).track_focus(this.handle)
      .on_key_down((event, cx) => this.key(event, cx)).child(formView(this, t));
    const panelFocused = this.panelFocused();
    const sections = this.sections();
    const rows = this.rows(sections);
    if (!rows.some(row => row.id === this.selected) && this.contextRow?.id !== this.selected &&
        (panelFocused || this.followedSelection !== this.selected ||
          !walk(this.data?.rows).some(row => row.kind === "task" && row.id === this.selected)))
      this.selected = rows.find(row => row.id === this.expanded)?.id ?? rows[0]?.id ?? null;
    const allSections = this.liveState().sections;
    const done = allSections.find(section => section.id === "done").rows.length;
    const open = allSections.filter(section => ["needs", "running", "progress", "todo"].includes(section.id))
      .reduce((count, section) => count + section.rows.filter(row => row.kind === "task").length, 0);
    const disabled = this.loading || this.busy;
    const list = v_flex().id("berbr-tasks").flex_1().min_h(0).overflow_y_scrollbar().px(6).py(4);
    if (!this.data && this.loading) {
      for (let index = 0; index < 5; index++) list.child(v_flex().h(44).flex_none().px(8).py(6).gap(6)
        .loop("opacity", {from:0.4, to:1, period:1200})
        .child(div().h(10).w("60%").rounded(4).bg(t.raised))
        .child(div().h(8).w("35%").rounded(4).bg(t.raised)));
    } else {
      if (this.filter.trim() && !sections.length) list.child(div().p(16).text_size(12).text_color(t.dim)
        .child(`Nothing matches “${this.filter.trim()}”`));
      if (this.data && !open && !this.filter.trim()) list.child(v_flex().p(16).gap(8).items_center()
        .child(div().text_size(13).text_color(t.muted).child("No open tasks"))
        .child(button(t, "empty-new", "New task", () => this.formOverlay("new"))));
      for (const section of sections) {
        const header = sectionHeader(t, section.id, section.label, section.rows.length, section.expanded,
          cx => this.toggleSection(section.id, cx), {selected:panelFocused && this.selected === `section-${section.id}`});
        if (section.id === "cleanup" && this.liveState().mergedTrees.length)
          header.child(button(t, "remove-merged", "Remove merged", () => this.formOverlay("remove-merged", {
            trees:this.liveState().trees,
          }), "ghost", disabled));
        list.child(header);
        if (section.expanded) for (const row of section.rows) list.child(row.kind === "task"
          ? this.taskBlock(t, row) : this.looseBlock(t, row, section.id));
      }
    }
    const sources = h_flex().min_w(0).gap(4).items_center().text_size(11);
    if (this.loading) sources.child(icon("spinner", 11).text_color(t.dim).spin(900))
      .child(div().text_color(t.dim).child("Syncing"));
    else for (const [index, name] of ["jira", "gitlab"].entries()) {
      const value = String(this.data?.sources?.[name] ?? "…").toLowerCase();
      if (index) sources.child(div().text_color(t.dim).child("·"));
      sources.child(div().min_w(0).truncate()
        .text_color(!this.data || ["cached", "online", "live", "ok"].includes(value) ? t.dim : t.blocked)
        .child(`${name === "jira" ? "Jira" : "GitLab"} ${value}`));
    }
    return v_flex().id("berbr").track_focus(this.handle).on_key_down((event, cx) => this.key(event, cx))
      .size_full().min_h(0).min_w(0).bg(t.surface).text_color(t.foreground)
      .child(h_flex().h(40).flex_none().px(12).gap(4).items_center().bg(t.chrome).border_b_1().border_color(t.border)
        .child(h_flex().id("project").h(26).min_w(0).gap(6).items_center().rounded(6).cursor_pointer()
          .hover(el => el.bg(t.hover)).on_click(() => this.formOverlay("projects", {projects:this.data?.projects ?? []}))
          .child(div().min_w(0).truncate().text_size(13).font_semibold().child(this.project ?? "Project"))
          .child(icon("chevron-down", 12).text_color(t.dim)))
        .child(div().flex_1())
        .child(iconButton(t, "refresh", "refresh", "Refresh (r)", cx => this.refresh(cx, false, true), {disabled, spinning:this.loading}))
        .child(iconButton(t, "new", "plus", "New task (n)", () => this.formOverlay("new"), {disabled})))
      .child(div().px(10).pt(8).pb(4).child(h_flex().h(30).px(8).gap(6).items_center()
        .rounded(6).bg(t.background).border_1().border_color(this.searchFocused ? t.accent : t.border)
        .child(icon("search").text_color(t.dim))
        .child(Input.new(this.inputs.search).flex_1().min_w(0).h(28).border_0().bg(t.background)
          .text_color(t.foreground).text_size(13))
        .when(!this.filter, el => el.child(kbd(t, "/")))
        .when(Boolean(this.filter), el => el.child(iconButton(t, "clear-search", "x", "Clear filter", cx => {
          this.filter = ""; this.values.search = ""; this.inputs.search.set_value(""); cx.notify();
        }, {size:22})))))
      .when(Boolean(this.error), el => el.child(this.banner(t, "dismiss-error", this.error, cx => {this.error = null; cx.notify();})))
      .when(Boolean(this.data?.warnings?.length), el => el.child(this.banner(t, "warnings", this.data.warnings.join("\n"))))
      .child(list)
      .child(h_flex().h(30).flex_none().px(12).gap(8).items_center().border_t_1().border_color(t.border)
        .child(div().id("toggle-done").text_size(11).text_color(this.all ? t.foreground : t.dim).cursor_pointer()
          .tooltip("Show completed tasks (d)").on_click((_event, cx) => {this.all = !this.all; cx.notify();}).child(`Done ${done}`))
        .child(div().flex_1())
        .child(panelFocused ? kbd(t, "⌘K jump · ↑↓ move · → expand · ⏎ open").min_w(0).truncate() : sources));
  }
}
