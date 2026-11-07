import { div, svg } from "gpui-kit";
import { h_flex, Switch } from "gpui-base";

export function icon(name, size = 14) {
  return svg(`mods/berbr/icons/${name}.svg`).size(size).flex_none();
}

export function pill(t, {label, tone = "dim", glyph, spin = false, tooltip = label}) {
  const color = t[tone] ?? t.dim;
  const tinted = typeof color === "string" && /^#[0-9a-f]{6}$/i.test(color);
  return h_flex().h(19).px(6).gap(4).flex_none().items_center().rounded(5).text_size(11)
    .bg(tinted ? color + (tone === "dim" ? "12" : "24") : t.raised).text_color(color)
    .tooltip(tooltip).accessibility_label(tooltip)
    .when(Boolean(glyph), el => el.child(["check", "spinner"].includes(glyph)
      ? icon(glyph, 11).when(spin, mark => mark.spin(900)) : div().flex_none().child(glyph)))
    .child(div().flex_none().text_color(color).child(label));
}

export function iconButton(t, id, name, label, action, { disabled = false, spinning = false, size = 26 } = {}) {
  return h_flex().id(id).size(size).flex_none().items_center().justify_center().rounded(6)
    .text_color(t.dim).cursor_pointer().tooltip(label).accessibility_label(label)
    .opacity(disabled && !spinning ? 0.4 : 1)
    .hover(el => el.bg(t.hover).text_color(t.foreground))
    .on_click((_event, cx) => { cx.stop_propagation(); if (!disabled) action(cx); })
    .child(icon(spinning ? "spinner" : name, spinning ? 11 : 14).when(spinning, el => el.spin(900)));
}

export function button(t, id, label, action, variant = "ghost", disabled = false, spinning = false) {
  const primary = variant === "primary";
  return keyboardControl(h_flex().id(id), action, disabled).h(30).px(10).gap(6).items_center().justify_center().rounded(6)
    .text_size(12).text_color(primary ? t.solid_foreground : t.muted)
    .when(primary, el => el.bg(t.solid)).cursor_pointer().opacity(disabled && !spinning ? 0.4 : 1)
    .hover(el => primary ? el.opacity(disabled && !spinning ? 0.4 : 0.85) : el.bg(t.hover).text_color(t.foreground))
    .on_click((_event, cx) => { cx.stop_propagation(); if (!disabled) action(cx); }).child(label)
    .child(div().size(11).flex_none().when(!spinning, el => el.invisible())
      .child(icon("spinner", 11).when(spinning, el => el.spin(900))));
}

// Plain elements need both a tab stop and keyboard activation in this runtime.
export function keyboardControl(element, action, disabled = false) {
  return element.tab_index(0).tab_stop(!disabled).on_key_down((event, cx) => {
    if (event.key === "enter" && event.modifiers?.platform) return;
    if (!["enter", "space", " "].includes(event.key)) return;
    cx.stop_propagation();
    if (!disabled) action(cx);
  });
}

export function chip(t, label) {
  const tone = label === "In review" ? t.accent
    : ["Closed", "Committed", "Merged"].includes(label) ? t.dim : t.muted;
  return div().flex_none().px(4).py(2).rounded(4).text_size(11).text_color(tone).child(label);
}

export function segmented(t, id, options, value, action, disabled = false) {
  return h_flex().h(28).p(2).gap(2).items_center().bg(t.background).border_1().border_color(t.border).rounded(6)
    .children(options.map(([key, label]) => keyboardControl(h_flex().id(`${id}-${key}`), cx => action(key, cx), disabled).h(22).px(10).items_center().rounded(4)
      .when(key === value, el => el.bg(t.raised)).text_size(12).text_color(key === value ? t.foreground : t.muted)
      .cursor_pointer().hover(el => el.text_color(t.foreground)).opacity(disabled ? 0.4 : 1)
      .on_click((_event, cx) => { if (!disabled) action(key, cx); }).child(label)));
}

export function sectionHeader(t, id, label, count, expanded = true, toggle = null, {selected = false} = {}) {
  return h_flex().id(`section-${id}`).h(28).flex_none().px(8).gap(6).items_center().text_size(12)
    .text_color(t.muted).font_medium().rounded(6).when(selected, el => el.bg(t.selected))
    .when(Boolean(toggle), el => el.cursor_pointer().hover(s => s.bg(selected ? t.selected : t.hover).text_color(t.foreground))
      .on_click((_event, cx) => {cx.stop_propagation(); toggle(cx);})
      .child(icon(expanded ? "chevron-down" : "chevron-right", 12).text_color(t.dim)))
    .child(label).child(div().text_color(id === "needs" && !expanded ? t.blocked : t.dim).child(String(count)));
}

export function statusMark(t, status) {
  const mark = h_flex().w(16).h(16).flex_none().items_center().justify_center();
  if (status === "working") return mark.child(icon("spinner", 12).text_color(t.working).spin(900));
  if (status === "closed") return mark.child(icon("check", 12).text_color(t.dim));
  if (["blocked", "done"].includes(status)) return mark.child(div().size(8).rounded(4).bg(t[status])
    .when(status === "blocked", el => el.loop("opacity", {from:0.35, to:1, period:1200})));
  return mark.child(div().size(8).rounded(4).border(1.5).border_color(["idea", "unknown"].includes(status) ? t.dim : t.muted));
}

export function kbd(t, text) {
  return div().text_size(11).text_color(t.dim).child(text);
}

export function worktreeSwitch(t, value, action, disabled, id = "worktree-switch") {
  return Switch.new(id).checked(value).disabled(disabled)
    .w(28).h(16).px(2).flex().items_center().rounded(8).cursor_pointer()
    .bg(value ? t.accent : t.raised).when(value, el => el.justify_end())
    .on_change(action).child(div().size(12).rounded(6).bg(t.foreground));
}
