import { div } from "gpui-kit";
import { h_flex, v_flex, Popup, TextView, EditorState, Editor } from "gpui-base";
import { props, closeOverlay, readTextFile, writeTextFile, watchFolder, settings } from "herdr";
import { exists } from "fs/promises";
import { parseFrontmatter, transformWikilinks, wikiTargets, wikiPath, obsidianUri, treeMarks, newTaskArgs, targetHint } from "./model.js";
import { icon, iconButton, button, chip, segmented, kbd, worktreeSwitch, keyboardControl } from "./ui.js";

function reloadNote(view, cx, {discard = false, links = false} = {}) {
  const note = view.note;
  if (note.saving || (note.editing && !discard)) return;
  const request = ++note.request, path = note.path;
  note.loading = true; cx.notify();
  cx.spawn(async cx => {
    try {
      if (!path) throw new Error("Note path is unavailable");
      const source = await readTextFile(path);
      if (source === null) throw new Error("File not found");
      if (request !== note.request || (note.editing && !discard)) return;
      if (!discard && !links && source === note.source && path === note.loadedPath) {
        note.loading = false; note.error = null; cx.notify(); return;
      }
      const parsed = parseFrontmatter(source);
      const existing = new Set();
      await Promise.all(wikiTargets(parsed.body).map(async target => {
        const destination = wikiPath(path, target);
        if (!destination) return;
        try { if (await exists(destination)) existing.add(destination); }
        catch (_) { /* Missing or unreadable links remain plain text. */ }
      }));
      if (request !== note.request || (note.editing && !discard)) return;
      note.source = source; note.properties = parsed.properties;
      note.loadedPath = path;
      note.text = transformWikilinks(parsed.body, path, existing).trim();
      note.existing = existing; note.error = null;
      if (discard) { note.editor?.set_value(source); note.dirty = false; note.conflict = false; note.ask = null; }
    } catch (failure) {
      if (request !== note.request) return;
      note.error = String(failure.message ?? failure);
    }
    if (request === note.request) { note.loading = false; cx.notify(); }
  });
}

function editNote(view, cx) {
  const note = view.note;
  if (note.loading || note.error || note.editing) return;
  note.editor?.release();
  note.editor = EditorState.new({language:"markdown", value:note.source,
    font_family:settings().font_family, font_size:14, soft_wrap:true, line_numbers:false, folding:false});
  note.editor.on("change", (_event, cx) => { note.dirty = note.editor.value() !== note.source; cx.notify(); });
  note.editing = true; note.ask = null; cx.notify();
}

function leaveNote(view, cx, close = false) {
  const note = view.note;
  if (note.saving) return;
  if (note.editing && note.dirty) { note.ask = close ? "close" : "view"; cx.notify(); return; }
  if (close) closeOverlay(props().overlay);
  else { note.editing = false; note.ask = null; view.handle.focus(); reloadNote(view, cx); }
}

function saveNote(view, cx, overwrite = false) {
  const note = view.note;
  if (!note.editing || note.saving || note.loading || (!note.dirty && !overwrite)) return;
  const value = note.editor.value(), path = note.path, original = note.source;
  note.saving = true; note.error = null; cx.notify();
  cx.spawn(async cx => {
    try {
      const expected = overwrite ? await readTextFile(path) : original;
      await writeTextFile(path, value, expected);
      note.source = value; note.loadedPath = null; note.dirty = note.editor.value() !== value; note.conflict = false; note.ask = null;
    } catch (failure) {
      let changed = false;
      try { changed = await readTextFile(path) !== original; } catch (_) { /* Report the write error. */ }
      if (changed) note.conflict = true;
      else note.error = String(failure.message ?? failure);
    }
    note.saving = false; cx.notify();
  });
}

export function noteKey(view, event, cx) {
  const note = view.note;
  if (event.key === "escape") leaveNote(view, cx, !note.editing);
  else if (event.modifiers?.platform && event.key.toLowerCase() === "s" && note.editing) saveNote(view, cx);
  else if (event.key === "e" && !note.editing && view.handle.is_focused() &&
      !event.modifiers?.platform && !event.modifiers?.control && !event.modifiers?.alt) editNote(view, cx);
  else return false;
  cx.stop_propagation(); return true;
}

export function initNote(view, cx) {
  const path = props()?.path ?? "";
  view.note = {path, source:"", text:"", properties:{}, existing:new Set(), error:null,
    loading:true, request:0, watchError:null, editing:false, dirty:false, saving:false};
  reloadNote(view, cx);
  if (!path) return;
  const folder = path.slice(0, path.lastIndexOf("/")) || "/";
  cx.spawn(async cx => {
    for (;;) {
      let paths;
      try { paths = await watchFolder(folder); }
      catch (error) { view.note.watchError = String(error.message ?? error); }
      cx.notify();
      if (!paths) return;
      if (!view.note.editing && (paths.includes(view.note.path) || paths.includes(folder))) reloadNote(view, cx);
    }
  });
}

function noteLink(view, url, cx) {
  if (/^https?:\/\//i.test(url)) { view.openUrl(url, cx); return; }
  if (!url.startsWith("berbr-note:")) return;
  let path;
  try { path = decodeURIComponent(url.slice("berbr-note:".length)); } catch (_) { return; }
  if (!view.note.existing.has(path)) return;
  view.note.path = path;
  reloadNote(view, cx);
}

function noteProperties(view, t, p) {
  const properties = view.note.properties;
  const header = h_flex().gap(8).items_center().flex_wrap().pb(12).text_size(12).text_color(t.dim);
  const values = value => Array.isArray(value) ? value : value ? [value] : [];
  if (properties.state || properties.status) header.child(chip(t, properties.state || properties.status));
  for (const tag of values(properties.tags)) header.child(div().child(`#${String(tag).replace(/^#/, "")}`));
  for (const key of ["date", "created", "updated", "started", "completed", "deadline"])
    if (typeof properties[key] === "string" && properties[key])
      header.child(div().tooltip(key.charAt(0).toUpperCase() + key.slice(1)).child(properties[key]));
  const passed = Array.isArray(p.tickets) ? p.tickets : Object.entries(p.tickets ?? {}).map(([key, url]) => ({key, url}));
  const tickets = new Set([...values(properties.tickets), ...passed.map(ticket => ticket.key ?? ticket.target)].filter(Boolean));
  for (const key of tickets) {
    const url = passed.find(ticket => (ticket.key ?? ticket.target) === key)?.url;
    const label = div().child(key);
    header.child(url && /^https?:\/\//i.test(url) ? label.id(`note-ticket-${key}`).text_color(t.accent).cursor_pointer()
      .on_click((_event, cx) => {cx.stop_propagation(); view.openUrl(url, cx);}) : label);
  }
  return header;
}

function noteMarkdown(view, t) {
  const content = v_flex().w_full().gap(12);
  const rich = (id, text) => TextView.markdown(id, text).selectable().text_size(14).line_height(1.55)
    .on_link_click((url, cx) => noteLink(view, url, cx));
  // Native builders provide spacing and code backgrounds; TextView handles inline Markdown.
  const blocks = view.note.text.split(/(^\s*```[^\n]*\n[\s\S]*?^\s*```\s*$|^\s*~~~[^\n]*\n[\s\S]*?^\s*~~~\s*$|^#{1,6}\s+[^\n]+$)/gm);
  blocks.forEach((text, index) => {
    if (!text.trim()) return;
    const id = `note-${view.note.path}-${index}`;
    const heading = text.match(/^(#{1,6})\s+(.+)$/);
    if (heading) content.child(rich(id, heading[2]).text_size([24, 20, 17, 15, 14, 14][heading[1].length - 1])
      .font_semibold().pt(heading[1].length < 3 ? 12 : 6).line_height(1.3));
    else if (/^\s*(```|~~~)/.test(text)) content.child(div().p(12).rounded(6).bg(t.background).child(rich(id, text)));
    else {
      let start = 0, number = 0;
      for (const match of text.matchAll(/`[^`\n]*`|!\[\[([^\]]*)\]\]/g)) {
        if (match[1] === undefined) continue;
        const before = text.slice(start, match.index).trim();
        if (before) content.child(rich(`${id}-${number++}`, before));
        content.child(div().px(8).py(4).rounded(4).bg(t.background).text_color(t.dim).text_size(12).child(match[1]));
        start = match.index + match[0].length;
      }
      const remaining = text.slice(start).trim();
      if (remaining) content.child(rich(`${id}-${number}`, remaining));
    }
  });
  return content;
}

function noteForm(view, t, p) {
  const note = view.note;
  const control = (id, name, label, action) => iconButton(t, id, name, label, action, {size:20, disabled:note.saving})
    .on_mouse_down("left", (_event, cx) => { view.handle.focus(); cx.stop_propagation(); });
  const body = v_flex().id("note-body").flex_1().min_h(0).overflow_y_scrollbar().px(24).py(16).text_size(14)
    .on_mouse_down("left", (_event, cx) => { if (!note.editing) view.handle.focus(); cx.stop_propagation(); });
  const content = v_flex().w_full().min_w(0).max_w(680);
  if (note.loading) content.child(div().text_color(t.dim).child("Loading note…"));
  else if (note.error && !note.editing) content.child(div().whitespace_normal().text_color(t.muted).child(`${note.path}: ${note.error}`));
  else if (!note.editing) {
    content.child(noteProperties(view, t, p));
    content.child(note.text ? noteMarkdown(view, t) : div().text_color(t.dim).child("Empty note"));
  }
  if (note.watchError) content.child(div().text_color(t.muted).child(`Live reload unavailable: ${note.watchError}`));
  return v_flex().size_full().min_h(0).bg(t.popover).text_color(t.foreground)
    .child(h_flex().h(40).flex_none().px(16).gap(8).items_center().border_b_1().border_color(t.border)
      .on_mouse_down("left", () => view.handle.focus())
      .child(div().flex_1().min_w(0).text_size(15).font_semibold().truncate()
        .child(`${note.path === p.path ? p.title ?? "Note" : note.path.split("/").pop().replace(/\.md$/, "")}${note.dirty ? " · Unsaved" : ""}`))
      .child(button(t, note.editing ? "note-save" : "note-edit", note.editing ? "Save" : "Edit",
        cx => note.editing ? saveNote(view, cx) : editNote(view, cx), "ghost", note.saving || note.loading || (!note.editing && Boolean(note.error)), note.saving)
        .on_mouse_down("left", (_event, cx) => cx.stop_propagation()))
      .when(note.editing, el => el.child(button(t, "note-done", "Done", cx => leaveNote(view, cx), "ghost", note.saving)
        .on_mouse_down("left", (_event, cx) => cx.stop_propagation())))
      // Obsidian only for a vault the snapshot named; berbr's own folder is the default
      .child(p.vault === true
        ? control("note-obsidian", "file-text", "Open in Obsidian", cx => view.openUrl(obsidianUri(note.path), cx))
        : control("note-open", "file-text", "Open in editor", cx => view.openUrl(note.path, cx)))
      .when(!note.editing, el => el.child(control("note-reload", "refresh", "Reload", cx => reloadNote(view, cx, {links:true}))))
      .child(control("note-close", "x", "Close", cx => leaveNote(view, cx, true))))
    .when(Boolean(note.ask), el => el.child(h_flex().px(16).py(8).gap(8).items_center()
      .child(div().flex_1().text_size(12).child("Unsaved changes"))
      .child(button(t, "note-keep-editing", "Keep editing", cx => {note.ask = null; cx.notify();}))
      .child(button(t, "note-discard", "Discard", cx => {
        const close = note.ask === "close"; note.dirty = false; note.ask = null; leaveNote(view, cx, close);
      }))))
    .when(note.conflict, el => el.child(h_flex().px(16).py(8).gap(8).items_center()
      .child(div().flex_1().text_size(12).text_color(t.blocked).child("Changed on disk"))
      .child(button(t, "note-conflict-reload", "Reload", cx => reloadNote(view, cx, {discard:true}), "ghost", note.saving || note.loading))
      .child(button(t, "note-overwrite", "Overwrite", cx => saveNote(view, cx, true), "ghost", note.saving || note.loading))))
    .child(note.editing ? div().flex_1().min_h(0).on_mouse_down("left", (_event, cx) => cx.stop_propagation())
      .child(Editor.new(note.editor).size_full()) : body.child(content))
    .when(Boolean(note.error && note.editing), el => el.child(div().px(16).py(8).text_color(t.blocked).child(note.error)))
    .when(Boolean(view.error), el => el.child(errorLine(view, t)));
}

function errorLine(view, t) {
  return div().text_size(12).text_color(t.blocked).child(view.error ?? "");
}

function cancel(view, t) {
  return button(t, "cancel", "Cancel", () => closeOverlay(props().overlay), "ghost", view.busy);
}

const sources = [["smart", "Generate", "search"], ["jira", "Jira", "file-text"], ["branch", "Branch", "git-branch"], ["name", "Name", "square-pen"]];

/** Popup paints above the dialog; its options never take space in the form. */
function choice(view, t, id, label, options, value, choose, placeholder, empty) {
  const open = view.choosing === id && !view.busy;
  const current = options.find(([key]) => key === value)?.[1] ?? value ?? placeholder;
  const toggle = cx => {
    if (view.busy) return;
    view.choosing = open ? null : id;
    view.choiceIndex = Math.max(0, options.findIndex(([key]) => key === value));
    (open ? view.choiceFocus[id] : view.choiceFocus.list).focus(); cx.notify();
  };
  const pick = (key, cx) => {
    if (view.busy) return;
    choose(key, cx); view.choosing = null;
    view.choiceFocus[id].focus(); cx.notify();
  };
  const trigger = keyboardControl(h_flex().id(`${id}-choice`), toggle, view.busy)
    .track_focus(view.choiceFocus[id]).h(34).w_full().px(10).gap(8).items_center().rounded(6)
    .border_1().border_color(open ? t.accent : t.border).bg(t.background)
    .on_size_change(({width}, cx) => {
      if (view.choiceWidths[id] !== width) { view.choiceWidths[id] = width; cx.notify(); }
    })
    .accessibility_label(label).opacity(view.busy ? 0.4 : 1).cursor_pointer().hover(el => el.bg(t.hover))
    .on_click((_event, cx) => { cx.stop_propagation(); toggle(cx); })
    .child(icon(id === "project" ? "folder" : "terminal", 14).text_color(t.dim))
    .child(div().flex_1().min_w(0).text_size(13).truncate().tooltip(current).child(current))
    .child(icon("chevron-down", 12).text_color(t.dim));
  const list = v_flex().id(`${id}-options`).track_focus(view.choiceFocus.list)
    .when(Boolean(view.choiceWidths[id]), el => el.w(view.choiceWidths[id]))
    .h(Math.min(Math.max(options.length, 1), 6) * 30 + 8).overflow_y_scrollbar().p(4).gap(2).rounded(6)
    .border_1().border_color(t.border).bg(t.popover).text_color(t.foreground)
    .on_mouse_down_out((_event, cx) => {
      if (view.choosing === id) { view.choosing = null; view.choiceFocus[id].focus(); cx.notify(); }
    })
    .on_key_down((event, cx) => {
      if (view.busy || !["up", "down", "enter"].includes(event.key) || event.modifiers?.platform) return;
      if (event.key === "enter") {
        const option = options[view.choiceIndex];
        if (option) pick(option[0], cx);
      } else if (options.length) {
        view.choiceIndex = (view.choiceIndex + (event.key === "down" ? 1 : -1) + options.length) % options.length;
        cx.notify();
      }
      cx.stop_propagation();
    })
    .children(options.map(([key, text], index) => keyboardControl(h_flex().id(`${id}-${key}`), cx => pick(key, cx), view.busy)
      .h(28).flex_none().px(8).gap(8).items_center().rounded(4).cursor_pointer()
      .when(index === view.choiceIndex, el => el.bg(t.selected)).hover(el => el.bg(t.hover))
      .on_click((_event, cx) => { cx.stop_propagation(); pick(key, cx); })
      .child(div().flex_1().min_w(0).text_size(13).truncate().tooltip(text).child(text))
      .when(key === value, el => el.child(icon("check", 12).text_color(t.dim)))))
    .when(!options.length, el => el.child(div().p(8).text_size(12).text_color(t.dim).child(empty)));
  return v_flex().w_full().flex_none().min_w(0).gap(6)
    .child(div().text_size(12).text_color(t.muted).child(label))
    .child(Popup.new(`${id}-popup`, trigger).w_full().anchor("bottom_left").when(open, el => el.content(list)));
}

function newForm(view, t) {
  const p = props() ?? {};
  const projects = (p.projects?.length ? p.projects : [view.project].filter(Boolean)).map(name => [name, name]);
  const tickets = view.newTaskTickets();
  const generate = view.source === "smart";
  const text = generate ? "" : view.values.title?.trim() ?? "";
  const plan = newTaskArgs({source:view.source, text, ticket:view.ticket, agent:view.agent, worktree:view.worktree,
    newTicket:view.newTicket, description:view.values.description});
  const neutralHint = {jira:"Pick a ticket below or paste a key or link",
    branch:"The branch is checked out if it exists, else created from origin.",
    name:targetHint({kind:"empty"})};
  const hint = plan.error ? neutralHint[view.source] : targetHint(plan.target, tickets, view.values.description);
  const setSource = (source, cx) => {
    if (view.busy) return;
    view.source = source; view.error = null; view.choosing = null;
    if (source !== "jira") view.ticket = null;
    view.inputs[source === "smart" ? "description" : "title"].focus(); cx.notify();
  };
  const tabs = h_flex().gap(16).items_center().border_b_1().border_color(t.border)
    .children(sources.map(([id, label, glyph]) => keyboardControl(h_flex().id(`source-${id}`), cx => setSource(id, cx), view.busy)
      .h(32).flex_none().gap(6).items_center().cursor_pointer().border_b_2()
      .opacity(view.busy ? 0.4 : 1)
      .border_color(view.source === id ? t.foreground : t.popover)
      .text_color(view.source === id ? t.foreground : t.muted).hover(el => el.text_color(t.foreground))
      .on_click((_event, cx) => setSource(id, cx))
      .child(icon(glyph, 13)).child(div().text_size(13).child(label))));
  const needle = text.toLowerCase();
  const shown = view.source === "jira"
    ? tickets.filter(ticket => !needle || ticket.key.toLowerCase().includes(needle) || ticket.title.toLowerCase().includes(needle))
    : [];
  const ticketList = view.source !== "jira" ? null : v_flex().id("new-tickets").flex_none()
    .h(Math.min(Math.max(shown.length, 1), 5) * 30).overflow_y_scrollbar().gap(2)
    .children(shown.map(ticket => keyboardControl(h_flex().id(`new-ticket-${ticket.key}`), cx => {
      view.ticket = view.ticket === ticket.key ? null : ticket.key; view.error = null; cx.notify();
    }, view.busy).h(28).flex_none().px(8).gap(8).items_center().rounded(4).cursor_pointer().opacity(view.busy ? 0.4 : 1).hover(el => el.bg(t.hover))
      .when(view.ticket === ticket.key, el => el.bg(t.selected))
      .on_click((_event, cx) => { if (!view.busy) { view.ticket = view.ticket === ticket.key ? null : ticket.key; view.error = null; cx.notify(); } })
      .child(div().flex_none().text_size(12).text_color(t.muted).child(ticket.key))
      .child(div().flex_1().min_w(0).text_size(13).truncate().tooltip(ticket.title).child(ticket.title))))
    .when(!shown.length, el => el.child(div().h(28).px(8).text_size(12).text_color(t.dim).truncate()
      .child(tickets.length ? "No matching tickets; a typed key works too" : "No open tickets without a task; type a key")));
  const toggle = (id, value, label, flip) => h_flex().flex_none().gap(8).items_center()
    .child(worktreeSwitch(t, value, (next, cx) => { if (!view.busy) { flip(next); view.error = null; cx.notify(); } }, view.busy, id)
      .tab_index(0).tab_stop(!view.busy).accessibility_label(label))
    .child(div().text_size(12).text_color(t.foreground).child(label));
  const scrollBody = view.source === "jira";
  const body = v_flex().id("new-body").min_h(0).gap(12)
    .when(scrollBody, el => el.flex_1().overflow_y_scrollbar())
    .when(!scrollBody, el => el.flex_none())
    .child(h_flex().id("new-pickers").flex_none().gap(12).items_start()
      .child(div().flex_1().min_w(0).child(choice(view, t, "project", "Project", projects, view.project,
        (name, cx) => view.chooseNewProject(name, cx), "Choose a project", "No projects available")))
      .child(div().flex_1().min_w(0).child(choice(view, t, "agent", "Agent",
        [["codex", "Codex"], ["claude", "Claude"], ["none", "None (a shell)"]], view.agent,
        (agent, cx) => { view.agent = agent; view.choosing = null; cx.notify(); }, "Choose an agent", "No agents available"))))
    .child(v_flex().flex_none().gap(8).child(tabs)
      .when(!generate, el => el.child(h_flex().h(36).gap(6).items_center()
        .child(icon("search", 14).text_color(t.dim))
        .child(div().flex_1().min_w(0).child(view.input("title").h(36))))
        .child(div().id("new-hint").text_size(11).text_color(t.dim).truncate().tooltip(hint).child(hint)))
      .when(Boolean(ticketList), el => el.child(ticketList)))
    .child(v_flex().flex_none().gap(6)
      .child(div().text_size(12).text_color(t.muted).child("Description"))
      .child(view.input("description").id("description")))
    .child(h_flex().id("new-options").flex_none().gap(16).items_center()
      .child(toggle("new-worktree", view.worktree, "Worktree", next => { view.worktree = next; }))
      .when(view.source !== "jira", el => el.child(toggle("new-jira-ticket", view.newTicket, "New Jira ticket", next => { view.newTicket = next; })))
      .when(Boolean(view.error) || view.created, el => el.child(div().id("new-status").flex_1().min_w(0).h(16)
        .when(Boolean(view.error), el => el.child(errorLine(view, t).truncate().tooltip(view.error)))
        .when(!view.error && view.created, el => el.child(div().text_size(12).text_color(t.done).child("Task created."))))));
  return v_flex().size_full().min_h(0).p(24).gap(16).line_height(1.25).bg(t.popover).text_color(t.foreground)
    .child(h_flex().h(28).flex_none().items_center()
      .child(div().flex_1().text_size(20).font_semibold().child("New task"))
      .child(keyboardControl(iconButton(t, "new-close", "x", "Close", () => closeOverlay(p.overlay), {disabled:view.busy}),
        () => closeOverlay(p.overlay), view.busy).tab_index(1)))
    .child(body)
    .child(v_flex().flex_none().gap(8)
      .child(h_flex().pt(12).gap(8).items_center().border_t_1().border_color(t.border)
        .child(toggle("create-more", view.createMore, "Create more", next => { view.createMore = next; }))
        .child(div().flex_1())
        .child(cancel(view, t))
        .child(button(t, "create", view.busyAction === "start" && plan.target?.kind === "empty" && view.values.description?.trim() ? "Naming…" : "Create task", cx => view.submitNew(cx), "primary", view.busy, view.busyAction === "start")
          .child(kbd(t, "⌘↵").px(4).py(2).rounded(4).border_1().border_color(t.border).text_color(t.solid_foreground)))));
}

function ticketForm(view, t, p) {
  return v_flex().size_full().min_h(0).p(24).gap(10).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child("Add a Jira ticket"))
    .child(div().text_size(12).text_color(t.dim).truncate().child(p.title ?? p.task ?? ""))
    .child(view.input("ticket"))
    .child(v_flex().id("ticket-worktree").flex_1().min_h(0).gap(6).overflow_y_scrollbar()
      .child(div().text_size(12).text_color(t.muted).child("New worktree name (optional)"))
      .child(view.input("name"))
      .when(Boolean(p.trees?.length), el => el.child(div().text_size(12).text_color(t.muted).child("Or reuse an existing worktree"))
        .children(p.trees.map(tree => button(t, `ticket-tree-${tree.id}`, `${view.ticketTree === tree.tree ? "✓ " : ""}${tree.tree}`,
          cx => {view.ticketTree = view.ticketTree === tree.tree ? null : tree.tree; cx.notify();}, "ghost", view.busy)
          .tooltip(tree.cwd ?? tree.tree))))
      .when(Boolean(p.tickets?.includes(view.values.ticket?.trim())), el => el.child(div().text_size(12).text_color(t.muted)
        .whitespace_normal().child("This ticket is already linked. Enter a name or select a worktree."))))
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
  if (view.planLoading) body.child(h_flex().gap(6).items_center().text_color(t.dim)
    .child(icon("spinner", 11).spin(900)).child(div().text_size(12).child("Checking close plan…")));
  section("Will be kept", (plan.keep ?? []).map(tree => row("git-branch", `${tree.tree} — ${tree.reason}`)));
  section("Left open in Jira", (plan.leave ?? []).map(key => row("file-text", key)));
  section("Will be removed", plan.removals.map(tree => row("git-branch",
    `${tree.name}${tree.path ? ` · ${tree.path}` : ""}${tree.changes ? ` — WARNING: deletes ${tree.changes} uncommitted file(s)` : ""}`, tree.changes ? t.blocked : t.foreground)));
  section("Jira", plan.jira.length ? [row("file-text", `Closes your open tickets: ${plan.jira.join(", ")}`),
    div().text_size(12).text_color(t.dim).whitespace_normal().child("Tickets assigned to someone else stay open.")] : []);
  section("Also", plan.also.map(text => row(text.includes("space") ? "monitor" : "file-text", text)));
  section("Proof", plan.proof.count ? [row("circle-check", `${plan.proof.count} MRs merged`, t.dim),
    view.externalLink(t, "close-proof", plan.proof.latest).text_size(13).text_color(t.dim)
      .child(div().min_w(0).truncate().child(plan.proof.latest.label))] : [], t.dim);
  const blocked = plan.blockers.length > 0;
  // the person may still close it, once told what is lost
  if (blocked && !view.planLoading) body.child(div().flex_none().text_size(12).text_color(t.blocked).whitespace_normal()
    .child("Close anyway skips these checks: the worktrees are deleted with their uncommitted files, "
      + "and their branches with commits found nowhere else. That is lost for good."));
  return v_flex().size_full().min_h(0).p(24).gap(16).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().whitespace_normal().child(`Close “${p.label ?? p.task ?? "task"}”?`))
    .child(body.when(Boolean(view.error), el => el.child(errorLine(view, t).whitespace_normal())))
    .child(h_flex().pt(12).gap(8).items_center().border_t_1().border_color(t.border)
      .child(div().flex_1().min_w(0).text_size(11).text_color(t.dim).whitespace_normal()
        .child("Berbr re-checks Jira and GitLab before closing."))
      .child(cancel(view, t))
      .when(blocked, el => el.child(button(t, "done-force", "Close anyway",
        cx => view.perform(cx, ["done", p.task, "--yes", "--force"], {close:true, action:"done-force"}), "ghost",
        view.busy || view.planLoading || Boolean(view.planError), view.busyAction === "done-force")))
      .child(button(t, "done", "Close task",
        cx => view.perform(cx, ["done", p.task, "--yes"], {close:true}), "primary", view.busy || view.planLoading || view.planError || blocked, view.busyAction === "done")
        .when(blocked, el => el.tooltip("Resolve the blockers first"))));
}

function removeMergedForm(view, t) {
  const plan = view.bulkPlan;
  const body = v_flex().flex_1().min_h(0).gap(10).overflow_y_scrollbar();
  if (view.planLoading) body.child(h_flex().gap(6).items_center().text_color(t.dim)
    .child(icon("spinner", 11).spin(900)).child(div().text_size(12).child("Checking worktrees…")));
  for (const blocker of plan.blockers) body.child(div().text_size(12).text_color(t.blocked).whitespace_normal().child(blocker));
  for (const tree of plan.remove) body.child(v_flex().gap(2)
    .child(div().text_size(13).child(tree.tree))
    .child(div().text_size(12).text_color(t.dim).whitespace_normal().child(tree.path ?? "")));
  if (plan.keep.length) body.child(div().text_size(12).text_color(t.muted).child("Will be kept"));
  for (const tree of plan.keep) body.child(div().text_size(12).text_color(t.dim).whitespace_normal().child(`${tree.tree} — ${tree.reason}`));
  if (!plan.remove.length && !view.planLoading) body.child(div().text_size(13).text_color(t.muted).child("No worktrees to remove"));
  if (view.error) body.child(errorLine(view, t).whitespace_normal());
  return v_flex().size_full().min_h(0).p(24).gap(16).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child("Remove merged worktrees?"))
    .child(body)
    .child(h_flex().gap(8).justify_end().child(cancel(view, t))
      .child(button(t, "confirm-remove-merged", `Remove ${plan.remove.length} worktrees`,
        cx => view.perform(cx, ["done", "--merged", "--yes"], {close:true}), "primary",
        view.busy || view.planLoading || view.planError || !plan.remove.length || Boolean(plan.blockers.length), view.busyAction === "done")));
}

function removeTreeForm(view, t, p) {
  const key = p.mr?.label?.match(/!\d+/)?.[0] ?? "";
  const state = p.mr?.state?.toLowerCase();
  const summary = p.mr ? `Its MR${key ? ` ${key}` : ""} is ${state}. ` : "";
  const marks = treeMarks(p.tree);
  // known blockers, or the CLI refused: the person may still choose to lose them
  const forceable = marks.changes > 0 || state === "opened" || p.tree?.mr_state === "opened" || Boolean(view.error);
  return v_flex().size_full().min_h(0).p(24).gap(12).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child(`Remove worktree “${p.tree?.tree ?? "unknown"}”?`))
    .child(v_flex().flex_1().min_h(0).gap(12).overflow_y_scrollbar()
      .child(div().text_size(13).text_color(t.muted).whitespace_normal()
        .child(`${summary}Deletes the checkout folder, its Herdr tabs, and the local branch when safe. Remove never discards uncommitted files.`))
      .child(div().text_size(12).text_color(t.muted).whitespace_normal()
        .child("Berbr refuses uncommitted changes, an open MR, unknown unpushed commits, or local commits not covered by a merged/closed MR. Unmerged commits already pushed to origin can be removed locally; the remote branch remains. Git and GitLab are checked again before removal."))
      .when(marks.changes > 0, el => el.child(div().text_size(12).text_color(t.blocked).whitespace_normal()
        .child(`Removal will be refused: ${marks.changes} uncommitted change(s) in the snapshot.`)))
      .when(state === "opened" || p.tree?.mr_state === "opened", el => el.child(div().text_size(12).text_color(t.blocked)
        .child("Removal will be refused: the MR is open.")))
      .when(Boolean(view.error), el => el.child(errorLine(view, t).whitespace_normal()))
      .when(forceable, el => el.child(div().text_size(12).text_color(t.blocked).whitespace_normal()
        .child(`Delete anyway skips these checks: ${marks.changes > 0 ? `${marks.changes} uncommitted change(s) are lost for good. ` : ""}`
          + "The local branch stays when it has commits found nowhere else."))))
    .child(h_flex().gap(8).justify_end().child(cancel(view, t))
      .when(forceable, el => el.child(button(t, "remove-tree-force", "Delete anyway", cx => view.submitRemoveTree(cx, {force:true}),
        "ghost", view.busy, view.busyAction === "remove-tree-force")))
      .child(button(t, "remove-tree", "Remove", cx => view.submitRemoveTree(cx),
        "primary", view.busy, view.busyAction === "remove-tree")));
}

function menuForm(view, t) {
  return v_flex().p(4).bg(t.popover).text_color(t.foreground)
    .children(view.menuItems.map((item, index) => v_flex()
      .when(Boolean(item.separatorBefore), el => el.child(div().h(1).my(4).bg(t.border)))
      .child(h_flex().id(`menu-${item.id}`).h(28).flex_none().px(10).items_center().rounded(5)
        .text_size(13).text_color(item.danger ? t.blocked : t.foreground).cursor_pointer()
        .when(index === view.menuIndex, el => el.bg(t.selected))
        .hover(el => el.bg(t.hover)).opacity(view.busy ? 0.4 : 1)
        .on_hover((hovered, cx) => { if (hovered && !view.busy) { view.menuIndex = index; cx.notify(); } })
        .on_click((_event, cx) => {cx.stop_propagation(); view.chooseMenu(cx, item.id);})
        .child(item.label))))
    .when(Boolean(view.error), el => el.child(div().p(8).child(errorLine(view, t).whitespace_normal())));
}

function renameForm(view, t) {
  return v_flex().size_full().p(24).gap(12).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child("Rename task"))
    .child(view.input("title"))
    .when(Boolean(view.error), el => el.child(errorLine(view, t).whitespace_normal()))
    .child(div().flex_1())
    .child(h_flex().gap(8).justify_end().child(cancel(view, t))
      .child(button(t, "rename-task", "Rename", cx => view.submitRename(cx), "primary", view.busy, view.busyAction === "rename")));
}

function stopAgentForm(view, t, p) {
  return v_flex().size_full().min_h(0).p(24).gap(12).bg(t.popover).text_color(t.foreground)
    .child(div().text_size(15).font_semibold().child("Stop agent?"))
    .child(div().text_size(13).text_color(t.muted).whitespace_normal()
      .child(`Stops terminal ${p.terminal ?? "unknown"}. Work in progress in this agent will be interrupted.`))
    .when(Boolean(view.error), el => el.child(errorLine(view, t).whitespace_normal()))
    .child(div().flex_1())
    .child(h_flex().gap(8).justify_end().child(cancel(view, t))
      .child(button(t, "stop-agent", "Stop agent", cx => view.submitStopAgent(cx), "primary", view.busy)));
}

// A popup is as tall as its content: a list that only grows (flex_1) gets no
// height at all, so the rows were there but invisible. Room for the rows, up to
// ten, and one line for "No matches".
export function pickerHeight(rows) {
  return Math.min(Math.max(rows, 1), 10) * 34 + 4;
}

function projectsForm(view, t, p) {
  const matches = view.pickerMatches(), start = Math.max(0, view.pickerIndex);
  return v_flex().size_full().min_h(0).p(4).gap(4).bg(t.popover).text_color(t.foreground)
    .child(view.input("search"))
    .child(v_flex().id("project-results").h(pickerHeight(matches.length - start)).flex_none().overflow_y_scrollbar()
      .children(matches.slice(start).map((project, index) => h_flex().id(`project-${project.name}`).h(32).flex_none().px(10).gap(8)
        .items_center().rounded(6).cursor_pointer().hover(el => el.bg(t.hover))
        .when(start + index === view.pickerIndex, el => el.bg(t.selected))
        .on_click((_event, cx) => view.choosePicker(cx, project))
        .child(div().flex_1().min_w(0).text_size(13).truncate().tooltip(project.name).child(project.name))
        .when(project.name === view.project, el => el.child(icon("check", 12).text_color(t.dim)))))
      .when(!matches.length, el => el.child(div().p(10).text_size(12).text_color(t.dim).child(p.projects?.length ? "No matches" : "No projects available"))))
    .child(kbd(t, "↑↓ move · ⏎ select · esc close"));
}

function jumpForm(view, t) {
  const matches = view.jumpMatches();
  const start = Math.max(0, view.jumpIndex);
  const names = {task:"Task", tree:"Worktree", agent:"Agent", ticket:"Ticket", mr:"MR", project:"Project"};
  return v_flex().size_full().min_h(0).p(6).gap(4).min_w(0).bg(t.popover).text_color(t.foreground)
    .child(view.input("search"))
    .child(v_flex().id("jump-results").flex_1().min_h(0).overflow_y_scrollbar().children(matches.slice(start, start + 10).map((item, index) =>
      h_flex().id(`jump-${item.id}`).h(30).flex_none().px(8).gap(6).items_center().min_w(0).rounded(6)
        .when(start + index === view.jumpIndex, el => el.bg(t.selected)).cursor_pointer().hover(el => el.bg(t.hover))
        .on_click((_event, cx) => view.chooseJump(cx, item))
        .child(icon({task:"circle-check", tree:"git-branch", agent:"terminal", ticket:"file-text", mr:"git-compare", project:"folder"}[item.kind], 12).text_color(t.dim))
        .child(div().flex_1().min_w(0).truncate().text_size(12).tooltip(item.title).child(item.title))
        .child(div().flex_none().text_size(11).text_color(t.dim).child(names[item.kind])))))
    .when(!matches.length, el => el.child(div().p(10).text_size(12).text_color(t.dim).child("No matches")))
    .child(kbd(t, "↑↓ move · ⏎ jump · esc close"));
}

function attachTaskForm(view, t, p) {
  const matches = view.pickerMatches(), start = Math.max(0, view.pickerIndex);
  return v_flex().size_full().min_h(0).p(4).gap(4).bg(t.popover).text_color(t.foreground)
    .child(view.input("search"))
    .child(v_flex().id("attach-results").h(pickerHeight(matches.length - start)).flex_none().overflow_y_scrollbar()
      .children(matches.slice(start).map((task, index) => {
        const id = `attach-task-${task.id}`, pending = view.busy && view.busyAction === id;
        return h_flex().id(id).h(32).flex_none().px(10).gap(8).items_center().rounded(6)
          .when(start + index === view.pickerIndex, el => el.bg(t.selected))
          .cursor_pointer().opacity(view.busy && !pending ? 0.4 : 1).hover(el => el.bg(t.hover))
          .on_click((_event, cx) => view.choosePicker(cx, task))
          .child(div().flex_1().min_w(0).truncate().tooltip(task.title ?? task.task).text_size(13).child(task.title ?? task.task))
          .child(div().size(11).flex_none().when(!pending, el => el.invisible()).child(icon("spinner", 11).text_color(t.dim).when(pending, el => el.spin(900))));
      }))
      .when(!matches.length, el => el.child(div().p(10).text_size(12).text_color(t.dim).child(p.tasks?.length ? "No matches" : "No open tasks"))))
    .when(Boolean(view.error), el => el.child(div().p(10).child(errorLine(view, t))))
    .child(kbd(t, "↑↓ move · ⏎ attach · esc close"));
}

export function formView(view, t) {
  const p = props() ?? {};
  if (view.form === "menu") return menuForm(view, t);
  if (view.form === "rename") return renameForm(view, t);
  if (view.form === "stop-agent") return stopAgentForm(view, t, p);
  if (view.form === "note") return noteForm(view, t, p);
  if (view.form === "new") return newForm(view, t);
  if (view.form === "ticket") return ticketForm(view, t, p);
  if (view.form === "projects") return projectsForm(view, t, p);
  if (view.form === "jump") return jumpForm(view, t);
  if (view.form === "attach-task") return attachTaskForm(view, t, p);
  if (view.form === "remove-merged") return removeMergedForm(view, t);
  if (view.form === "remove-tree") return removeTreeForm(view, t, p);
  return doneForm(view, t, p);
}
