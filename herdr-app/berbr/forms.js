import { div } from "gpui-kit";
import { h_flex, v_flex, TextView } from "gpui-base";
import { props, closeOverlay, setShared, readTextFile, watchFolder } from "herdr";
import { cleanObsidian, stripFrontmatter, obsidianUri } from "./model.js";
import { icon, iconButton, button, segmented, kbd, worktreeSwitch } from "./ui.js";

function reloadNote(view, cx) {
  const note = view.note;
  const request = ++note.request;
  note.loading = true; cx.notify();
  cx.spawn(async cx => {
    let text = "", error = null;
    try {
      if (!note.path) throw new Error("Note path is unavailable");
      const source = await readTextFile(note.path);
      if (source === null) throw new Error("File not found");
      text = cleanObsidian(stripFrontmatter(source));
    } catch (failure) { error = String(failure.message ?? failure); }
    if (request !== note.request) return;
    note.text = text; note.error = error; note.loading = false;
    cx.notify();
  });
}

export function initNote(view, cx) {
  const path = props()?.path ?? "";
  view.note = {path, text:"", error:null, loading:true, request:0, watchError:null};
  reloadNote(view, cx);
  if (!path) return;
  const folder = path.slice(0, path.lastIndexOf("/")) || "/";
  cx.spawn(async cx => {
    for (;;) {
      let paths;
      try { paths = await watchFolder(folder); }
      catch (error) { view.note.watchError = String(error.message ?? error); }
      // This also ends the loop when its copy has closed or been replaced.
      cx.notify();
      if (!paths) return;
      if (paths.includes(path) || paths.includes(folder)) reloadNote(view, cx);
    }
  });
}

function noteForm(view, t, p) {
  const note = view.note;
  const control = (id, name, label, action) => iconButton(t, id, name, label, action, {size:20})
    .on_mouse_down("left", (_event, cx) => { view.handle.focus(); cx.stop_propagation(); });
  const body = v_flex().id("note-body").flex_1().min_h(0).overflow_y_scrollbar().px(24).py(16).text_size(13)
    .on_mouse_down("left", (_event, cx) => { view.handle.focus(); cx.stop_propagation(); });
  const content = v_flex().w_full().min_w(0).max_w(680);
  if (note.loading) content.child(div().text_color(t.dim).child("Loading note…"));
  else if (note.error) content.child(div().whitespace_normal().text_color(t.muted).child(`${note.path}: ${note.error}`));
  else if (!note.text) content.child(div().text_color(t.dim).child("Empty note"));
  else content.child(TextView.markdown(`note-${note.path}`, note.text).selectable().text_size(13));
  if (note.watchError) content.child(div().text_color(t.muted).child(`Live reload unavailable: ${note.watchError}`));
  return v_flex().size_full().min_h(0).bg(t.popover).text_color(t.foreground)
    .on_mouse_down("left", () => view.handle.focus())
    .child(h_flex().h(40).flex_none().px(16).gap(8).items_center().border_b_1().border_color(t.border)
      .child(div().flex_1().min_w(0).text_size(15).font_semibold().truncate().child(p.title ?? "Note"))
      .child(control("note-obsidian", "file-text", "Open in Obsidian", cx => view.openUrl(obsidianUri(note.path), cx)))
      .child(control("note-reload", "refresh", "Reload", cx => reloadNote(view, cx)))
      .child(control("note-close", "x", "Close", () => closeOverlay(props().overlay))))
    .child(body.child(content))
    .when(Boolean(view.error), el => el.child(errorLine(view, t)));
}

function errorLine(view, t) {
  return div().text_size(12).text_color(t.blocked).child(view.error ?? "");
}

function cancel(view, t) {
  return button(t, "cancel", "Cancel", () => closeOverlay(props().overlay), "ghost", view.busy);
}

function field(t, label, control) {
  return h_flex().gap(12).items_center()
    .child(div().w(84).flex_none().text_size(12).text_color(t.muted).child(label)).child(control);
}

function newForm(view, t) {
  const body = v_flex().flex_1().min_h(0).gap(12).overflow_y_scrollbar()
    .child(v_flex().gap(4).child(view.input("title"))
      .child(div().text_size(11).text_color(t.dim).child("Leave empty and the agent will name it.")))
    .child(field(t, "Agent", segmented(t, "agent", [["codex", "Codex"], ["claude", "Claude"], ["none", "None"]],
      view.agent, (value, cx) => { view.agent = value; cx.notify(); }, view.busy)))
    .child(field(t, "Jira", h_flex().gap(8).items_center().flex_1().min_w(0)
      .child(segmented(t, "jira", [["none", "None"], ["new", "New ticket"], ["existing", "Existing"]],
        view.ticketMode, (value, cx) => { view.ticketMode = value; cx.notify(); }, view.busy))
      .when(view.ticketMode === "existing", el => el.child(div().flex_1().min_w(0).child(view.input("ticket"))))))
    .child(field(t, "Worktree", h_flex().gap(8).items_center()
      .child(worktreeSwitch(t, view.worktree, (value, cx) => {view.worktree = value; cx.notify();}, view.busy))
      .child(div().text_size(12).text_color(t.foreground).child("Create a worktree"))))
    .when(view.worktree, el => el.child(v_flex().gap(8)
      .child(h_flex().id("advanced").h(28).gap(6).items_center().cursor_pointer().text_color(t.muted)
        .hover(el => el.text_color(t.foreground))
        .on_click((_event, cx) => {view.advanced = !view.advanced; cx.notify();})
        .child(icon(view.advanced ? "chevron-down" : "chevron-right", 12))
        .child(div().text_size(12).child("Advanced")))
      .when(view.advanced, el => el.child(v_flex().gap(8)
        .child(field(t, "Worktree name", div().flex_1().min_w(0).child(view.input("name"))))
        .child(field(t, "Branch", div().flex_1().min_w(0).child(view.input("branch"))))))));
  return v_flex().size_full().min_h(0).p(24).gap(16).bg(t.popover).text_color(t.foreground)
    .child(h_flex().items_center().justify_between()
      .child(div().text_size(20).font_semibold().child("New task"))
      .child(div().text_size(12).text_color(t.dim).child(`in ${view.project ?? "project"}`)))
    .child(body)
    .when(Boolean(view.error), el => el.child(errorLine(view, t)))
    .child(h_flex().pt(12).gap(8).items_center().border_t_1().border_color(t.border)
      .child(kbd(t, "enter create · esc cancel")).child(div().flex_1())
      .child(cancel(view, t))
      .child(button(t, "create", "Create task", cx => view.submitNew(cx), "primary", view.busy, view.busyAction === "start")));
}

function ticketForm(view, t, p) {
  return v_flex().size_full().p(24).gap(10).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child("Add a Jira ticket"))
    .child(div().text_size(12).text_color(t.dim).truncate().child(p.title ?? p.task ?? ""))
    .child(view.input("ticket"))
    .when(Boolean(view.error), el => el.child(errorLine(view, t)))
    .child(div().flex_1())
    .child(h_flex().gap(8).justify_end()
      .child(button(t, "attach", "Link only", cx => view.submitTicket(cx, false), "ghost", view.busy, view.busyAction === "attach"))
      .child(button(t, "add", "Add with worktree", cx => view.submitTicket(cx, true), "primary", view.busy, view.busyAction === "add")));
}

function doneForm(view, t, p) {
  const plan = view.closePlan;
  const body = v_flex().flex_1().min_h(0).gap(16).overflow_y_scrollbar();
  const row = (name, text, tone = t.foreground) => h_flex().flex_none().gap(8).items_start().text_color(tone)
    .child(icon(name, 13)).child(div().flex_1().min_w(0).text_size(13).whitespace_normal().child(text));
  const section = (title, rows, tone = t.muted) => {
    if (rows.length) body.child(v_flex().flex_none().gap(6)
      .child(div().text_size(12).text_color(tone).child(title)).children(rows));
  };
  section("Blocks closing", plan.blockers.map(text => row("x", text, t.blocked)), t.blocked);
  section("Will be removed", plan.removals.map(tree => row("git-branch",
    `${tree.name}${tree.changes ? ` — loses +${tree.changes} uncommitted` : ""}`, tree.changes ? t.blocked : t.foreground)));
  section("Jira", plan.jira.length ? [row("file-text", `Closes your open tickets: ${plan.jira.join(", ")}`),
    div().text_size(12).text_color(t.dim).whitespace_normal().child("Tickets assigned to someone else stay open.")] : []);
  section("Also", plan.also.map(text => row(text.includes("space") ? "monitor" : "file-text", text)));
  section("Proof", plan.proof.count ? [row("circle-check", `${plan.proof.count} MRs merged`, t.dim),
    view.externalLink(t, "close-proof", plan.proof.latest).text_size(13).text_color(t.dim)
      .child(div().min_w(0).truncate().child(plan.proof.latest.label))] : [], t.dim);
  const blocked = plan.blockers.length > 0;
  return v_flex().size_full().min_h(0).p(24).gap(16).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().whitespace_normal().child(`Close “${p.label ?? p.task ?? "task"}”?`))
    .child(body.when(Boolean(view.error), el => el.child(errorLine(view, t).whitespace_normal())))
    .child(h_flex().pt(12).gap(8).items_center().border_t_1().border_color(t.border)
      .child(div().flex_1().min_w(0).text_size(11).text_color(t.dim).whitespace_normal()
        .child("Berbr re-checks Jira and GitLab before closing."))
      .child(cancel(view, t))
      .child(button(t, "done", "Close task",
        cx => view.perform(cx, ["done", p.task, "--yes"], {close:true}), "primary", view.busy || blocked, view.busyAction === "done")
        .when(blocked, el => el.tooltip("Resolve the blockers first"))));
}

function removeTreeForm(view, t, p) {
  const key = p.mr?.label?.match(/!\d+/)?.[0] ?? "";
  const state = p.mr?.state?.toLowerCase();
  const summary = p.mr ? `Its MR${key ? ` ${key}` : ""} is ${state}. ` : "";
  return v_flex().size_full().min_h(0).p(24).gap(12).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child(`Remove worktree “${p.tree?.tree ?? "unknown"}”?`))
    .child(v_flex().flex_1().min_h(0).gap(12).overflow_y_scrollbar()
      .child(div().text_size(13).text_color(t.muted).whitespace_normal()
        .child(`${summary}The checkout folder is deleted. Berbr refuses if it has uncommitted changes, unpushed commits or an open MR.`))
      .when(Boolean(view.error), el => el.child(errorLine(view, t).whitespace_normal())))
    .child(h_flex().gap(8).justify_end().child(cancel(view, t))
      .child(button(t, "remove-tree", "Remove", cx => view.submitRemoveTree(cx),
        "primary", view.busy, view.busyAction === "remove-tree")));
}

function projectsForm(view, t, p) {
  return v_flex().p(4).bg(t.popover).text_color(t.foreground)
    .children((p.projects ?? []).map(project => h_flex().id(`project-${project.name}`).h(32).flex_none().px(10).gap(8)
      .items_center().rounded(6).cursor_pointer().hover(el => el.bg(t.hover))
      .on_click(() => {setShared("berbr.project", project.name); closeOverlay(p.overlay);})
      .child(div().flex_1().min_w(0).text_size(13).truncate().child(project.name))
      .when(project.name === view.project, el => el.child(icon("check", 12).text_color(t.dim)))))
    .when(!p.projects?.length, el => el.child(div().p(10).text_size(12).text_color(t.dim).child("No projects available")));
}

function attachTaskForm(view, t, p) {
  return v_flex().p(4).bg(t.popover).text_color(t.foreground)
    .children((p.tasks ?? []).map(task => {
      const id = `attach-task-${task.id}`;
      const pending = view.busy && view.busyAction === id;
      return h_flex().id(id).h(32).flex_none().px(10).gap(8).items_center().rounded(6)
        .cursor_pointer().opacity(view.busy && !pending ? 0.4 : 1).hover(el => el.bg(t.hover))
        .on_click((_event, cx) => {
          if (!p.ticket || !task.task) { view.error = "Ticket or task name is unavailable."; cx.notify(); return; }
          view.perform(cx, ["attach", p.ticket, task.task], {close:true, action:id});
        })
        .child(div().flex_1().min_w(0).truncate().text_size(13).child(task.title ?? task.task))
        .when(pending, el => el.child(icon("spinner", 11).text_color(t.dim).spin(900)));
    }))
    .when(!p.tasks?.length, el => el.child(div().p(10).text_size(12).text_color(t.dim).child("No open tasks")))
    .when(Boolean(view.error), el => el.child(div().p(10).child(errorLine(view, t))));
}

export function formView(view, t) {
  const p = props() ?? {};
  if (view.form === "note") return noteForm(view, t, p);
  if (view.form === "new") return newForm(view, t);
  if (view.form === "ticket") return ticketForm(view, t, p);
  if (view.form === "projects") return projectsForm(view, t, p);
  if (view.form === "attach-task") return attachTaskForm(view, t, p);
  if (view.form === "remove-tree") return removeTreeForm(view, t, p);
  return doneForm(view, t, p);
}
