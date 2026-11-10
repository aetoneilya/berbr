import {div} from "gpui-kit";
import {h_flex, v_flex, Popup} from "gpui-base";
import {icon, keyboardControl, worktreeSwitch} from "./ui.js";
import {scopeSummary} from "./scope.js";

export function boardFilters(view, t) {
  const open = view.filtersOpen;
  const projects = view.data?.projects ?? [];
  const selected = view.scopeProjects ?? [view.project].filter(Boolean);
  const summary = scopeSummary(view.scopeProjects, projects, view.project);
  const completed = view.view === "mrs" ? view.mrAll : view.all;
  const completedCount = view.view === "mrs" ? view.liveState().mrs.filter(mr => mr.completed).length
    : view.liveState().sections.find(section => section.id === "done").rows.length;
  const active = view.view === "mrs" || completed || view.scopeProjects !== null;
  const toggle = cx => {
    if (view.busy) return;
    view.filtersOpen = !open;
    (view.filtersOpen ? view.filtersFocus : view.handle).focus(); cx.notify();
  };
  const trigger = keyboardControl(h_flex().id("filters"), toggle, view.busy)
    .size(30).flex_none().items_center().justify_center().rounded(6).cursor_pointer()
    .bg(view.filtersOpen || active ? t.raised : t.surface).text_color(active ? t.foreground : t.dim)
    .hover(el => el.bg(t.hover).text_color(t.foreground))
    .tooltip(`Filters · ${view.view === "mrs" ? "MR" : "Tasks"} · ${summary}`)
    .accessibility_label("Filters").on_click((_event, cx) => {cx.stop_propagation(); toggle(cx);})
    .on_hover(hovered => {view.filtersTriggerHovered = hovered;})
    .child(icon("sliders", 16));
  const choose = action => cx => {action(cx); view.filtersFocus.focus(); cx.notify();};
  const option = (id, label, checked, action) => keyboardControl(h_flex().id(id), choose(action), view.busy)
    .h(30).flex_none().px(7).gap(8).items_center().rounded(4).cursor_pointer()
    .hover(el => el.bg(t.hover)).on_click((_event, cx) => {cx.stop_propagation(); if (!view.busy) choose(action)(cx);})
    .accessibility_label(`${label}${checked ? ", selected" : ""}`)
    .child(div().size(14).flex_none().rounded(3).border_1().border_color(checked ? t.accent : t.border)
      .when(checked, el => el.child(icon("check", 12).text_color(t.accent))))
    .child(div().flex_1().min_w(0).truncate().text_size(12).child(label));
  const heading = label => div().px(7).pt(8).pb(4).text_size(11).font_medium().text_color(t.dim).child(label);
  const separator = () => div().mx(7).my(6).h(1).flex_none().bg(t.border);
  const expand = section => choose(() => {view.filtersSection = view.filtersSection === section ? null : section;});
  const disclosure = (id, label, value, section) => keyboardControl(h_flex().id(id), expand(section), view.busy)
    .h(34).px(7).gap(8).items_center().rounded(4).cursor_pointer().hover(el => el.bg(t.hover))
    .on_click((_event, cx) => {cx.stop_propagation(); if (!view.busy) expand(section)(cx);})
    .child(div().flex_1().min_w(0).text_size(12).child(label))
    .child(div().max_w("55%").min_w(0).truncate().text_size(12).text_color(t.dim).tooltip(value).child(value))
    .child(icon(view.filtersSection === section ? "chevron-down" : "chevron-right", 11).text_color(t.dim));
  const menu = v_flex().id("board-filters").w(292).max_h(520).overflow_y_scrollbar().p(8).gap(2)
    .rounded(10).bg(t.popover).border_1().border_color(t.border).text_color(t.foreground)
    .track_focus(view.filtersFocus)
    .on_mouse_down_out((_event, cx) => {if (view.filtersOpen && !view.filtersTriggerHovered) {view.filtersOpen = false; cx.notify();}})
    .on_key_down((event, cx) => {if (event.key === "escape") {view.filtersOpen = false; view.handle.focus(); cx.stop_propagation(); cx.notify();}})
    .child(div().px(7).pt(5).pb(4).text_size(14).font_semibold().child("View options"))
    .child(heading("Show"))
    .child(disclosure("filter-view", "View", view.view === "mrs" ? "Merge requests" : "Tasks", "view"))
    .when(view.filtersSection === "view", el => el.children([["tasks", "Tasks"], ["mrs", "Merge requests"]].map(([key, label]) =>
      option(`view-${key}`, label, view.view === key, cx => view.switchView(key, cx)))))
    .child(disclosure("filter-projects", "Projects", summary, "projects"))
    .when(view.filtersSection === "projects", el => el.child(v_flex().id("filter-project-list")
      .h(Math.min(projects.length + 1, 7) * 30).overflow_y_scrollbar()
      .child(option("filter-all-projects", "All projects", projects.length > 0 && projects.every(project => selected.includes(project.name)), cx =>
        view.setProjectFilter(projects.every(project => (view.scopeProjects ?? [view.project]).includes(project.name)) ? [] : projects.map(project => project.name), cx)))
      .children(projects.map(project => option(`filter-project-${project.name}`, project.name, selected.includes(project.name), cx => {
        const current = view.scopeProjects ?? [view.project].filter(Boolean);
        view.setProjectFilter(current.includes(project.name) ? current.filter(name => name !== project.name) : [...current, project.name], cx);
      })))))
    .child(separator()).child(heading("Filters"))
    .child(h_flex().id("toggle-done").min_h(36).px(7).gap(10).items_center()
      .child(div().flex_1().min_w(0).text_size(12).child("Show completed"))
      .child(div().text_size(11).text_color(t.dim).child(String(completedCount)))
      .child(worktreeSwitch(t, completed, (_checked, cx) => view.toggleCompleted(cx), view.busy, "filter-completed")));
  return Popup.new("board-filters-popup", trigger).anchor("top_right")
    .when(view.filtersOpen, el => el.content(menu));
}
