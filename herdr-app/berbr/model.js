export function walk(rows = []) {
  return (rows ?? []).flatMap(row => [row, ...walk(row.children)]);
}

export function worktreeNameCollision(rows, name) {
  const key = String(name ?? "").trim().toLowerCase();
  return Boolean(key) && walk(rows).some(row => row.kind === "task" && !row.done &&
    [row.task, row.title].some(value => String(value ?? "").trim().toLowerCase() === key));
}

export function cleanCliText(text) {
  return String(text ?? "").replace(/\(eval\):\d+: can't change option: zle/g, "").trim();
}

export function agentActivity(title, app, treeName) {
  let text = String(title ?? "").trim().replace(/^[✳✻✽✶✢✦✧✺•·●○◐◓◑◒⠁-⣿\uFE0F\s]+/u, "").trim();
  const name = String(app ?? "").trim().replace(/[-_]+/g, " ");
  const names = ["claude", "claude code"].includes(name.toLowerCase()) ? ["Claude Code", "Claude"] : [name];
  for (const prefix of names.filter(Boolean)) {
    const start = text.slice(0, prefix.length).replace(/[-_]/g, " ");
    if (start.toLowerCase() === prefix.toLowerCase() &&
        (!text[prefix.length] || /^[\s:—–-]/.test(text.slice(prefix.length)))) {
      text = text.slice(prefix.length).replace(/^[\s:—–-]+/, "").trim();
      break;
    }
  }
  const tree = String(treeName ?? "").trim();
  if (tree) {
    const suffix = tree.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    text = text.replace(new RegExp(`\\s+(?:\\||—|·)\\s+${suffix}\\s*$`, "i"), "").trim();
  }
  return text;
}

export function formatElapsed(ms) {
  if (ms === null || ms === undefined || !Number.isFinite(ms)) return "";
  const minutes = Math.floor(Math.max(0, ms) / 60000);
  return minutes < 60 ? `${minutes}m` : `${Math.floor(minutes / 60)}h${minutes % 60 ? ` ${minutes % 60}m` : ""}`;
}

export function classifyBlocked(screenText) {
  const lines = String(screenText ?? "").replace(/\x1b\[[0-?]*[ -/]*[@-~]/g, "")
    .split(/\r?\n/).map(line => line.trim().replace(/^[│┃║❯> ]+|[│┃║ ]+$/g, "").trim()).filter(Boolean);
  const permission = line => /^(?:do you want to (?:proceed|make this edit)|allow\b.*\?|approve(?:\s+(?:command|this command|this action|execution))?\s*[?:]?$|run the following command\b)/i.test(line);
  for (let index = lines.length - 1; index >= 0; index--) {
    if (!permission(lines[index])) continue;
    const previous = lines[index - 1] ?? "";
    const command = /^run the following command\b/i.test(lines[index])
      ? lines[index].replace(/^run the following command\s*:?\s*/i, "") : "";
    const detail = command || (/^(?:[$>]\s*\S|(?:Bash|Edit|Write|Read)\s*\(|(?:git|npm|node|cargo|python\d*|rg|ls|cd)\s|(?:[~/.]|[\w.-]+\/)[^\s]*|[\w.-]+\.[\w]+\b)/i.test(previous)
      ? previous.replace(/^[$>]\s*/, "") : "");
    return detail ? {kind:"permission", detail:detail.slice(0, 80)} : {kind:"permission"};
  }
  const question = [...lines].reverse().find(line => line.endsWith("?") && !permission(line));
  return question ? {kind:"question", detail:question} : {kind:"waiting"};
}

export function closePlan(row, liveSpaces = [], related = walk(row?.children)) {
  const unique = items => [...new Map(items.map(item => [item.url || item.cwd || item.id || item.target || item.tree || item, item])).values()];
  const mrs = unique(related.filter(item => item.kind === "mr"));
  const trees = unique(related.filter(item => item.kind === "tree"));
  const removals = trees.map(tree => ({name:tree.tree ?? tree.label ?? "Worktree",
    ...parseTreeMarks(tree.body, tree.state)}));
  const blockers = mrs.filter(mr => ["opened", "open"].includes(mr.state?.toLowerCase()))
    .map(mr => `MR ${mr.label?.match(/!\d+/)?.[0] ?? `!${mr.url?.match(/\/(\d+)\/?$/)?.[1] ?? mr.target ?? "?"}`} is open`);
  blockers.push(...removals.filter(tree => tree.unpushed).map(tree => `${tree.name}: ↑${tree.unpushed} unpushed`));
  // Snapshot tickets have no assignee; the CLI re-checks ownership before closing.
  const jira = [...new Set(related.filter(item => item.kind === "ticket" && !isClosedTicket(item))
    .map(ticket => ticket.target).filter(Boolean))];
  const also = ["Marks the note done"];
  if (row?.space && liveSpaces.some(space => space.id === row.space)) also.push("Closes the Herdr space");
  // Snapshots have no merge timestamps. MR iid is the stable fallback order.
  const merged = mrs.filter(mr => mr.state?.toLowerCase() === "merged").sort((a, b) =>
    Number(b.label?.match(/!(\d+)/)?.[1] ?? 0) - Number(a.label?.match(/!(\d+)/)?.[1] ?? 0));
  return {blockers, removals, jira, also, proof:{count:merged.length, latest:merged[0] ?? null}};
}

export function normalizeTicketState(state) {
  const value = String(state ?? "").trim();
  const normalized = {
    "новый": "New", "new": "New", "open": "New", "to do": "New",
    "в работе": "In progress", "in progress": "In progress",
    "on review": "In review", "in review": "In review", "review": "In review",
    "commited": "Committed", "committed": "Committed",
    "закрыт": "Closed", "closed": "Closed", "done": "Closed", "resolved": "Closed",
  }[value.toLowerCase()];
  return normalized ?? (value.charAt(0).toUpperCase() + value.slice(1).toLowerCase());
}

export function isClosedTicket(row) {
  return ["Closed", "Committed"].includes(normalizeTicketState(row.state));
}

export function parseMrFlags(body = "") {
  const tokens = String(body ?? "").trim().toLowerCase().split(/\s+/);
  const ci = tokens.includes("ci✗") ? "failed"
    : tokens.includes("ci✓") ? "passed" : tokens.includes("ci…") ? "running" : null;
  const conflict = tokens.includes("conflict");
  const draft = tokens.includes("draft");
  const age = tokens.find(token => /^\d+d$/.test(token));
  return {
    ci, conflict, draft, age: age ? Number(age.slice(0, -1)) : null,
    worst: ci === "failed" ? "CI failed" : conflict ? "conflict" : draft ? "draft" : null,
  };
}

export function parseTreeMarks(body = "", state = "") {
  const lines = String(body ?? "").split("\n");
  const first = lines[0].trim();
  const onlyMarks = /^(?:\+\d+|↑\d+|✓\s+merged|quiet\s+\d+d)(?:\s|$)/.test(first);
  const path = onlyMarks ? "" : first;
  const marks = onlyMarks ? lines.join(" ") : lines.slice(1).join(" ");
  const quiet = marks.match(/(?:^|\s)quiet\s+(\d+)d(?:\s|$)/);
  return {
    path,
    changes:Number(marks.match(/(?:^|\s)\+(\d+)(?=\s|$)/)?.[1] ?? 0),
    unpushed:Number(marks.match(/(?:^|\s)↑(\d+)(?=\s|$)/)?.[1] ?? 0),
    merged:/(?:^|\s)✓\s+merged(?:\s|$)/.test(marks) || state?.toLowerCase() === "finished",
    quietDays:quiet ? Number(quiet[1]) : null,
  };
}

export function looseWhy(row) {
  if (isMergedTree(row) || row.children?.some(isMergedTree)) return "The branch is merged; its worktree is still on disk.";
  if (row.kind === "ticket") {
    const tree = row.children?.find(child => child.kind === "tree");
    const mr = walk(row.children).find(child => child.kind === "mr" && child.state?.toLowerCase() === "opened");
    const key = row.target ?? "this ticket";
    const needle = String(row.target ?? "").trim().toLowerCase();
    const mentions = text => Boolean(needle) && String(text ?? "").toLowerCase().includes(needle);
    const treeMr = tree?.children?.find(child => child.kind === "mr" && mentions(child.label));
    const mrKey = treeMr?.label?.match(/!\d+/)?.[0];
    const relation = mentions(tree?.branch)
      ? `is on branch ${tree.branch}, which mentions ${key}.`
      : mrKey ? `has MR ${mrKey}, whose title mentions ${key}.` : `is linked to ${key}.`;
    const why = tree
      ? `Your worktree ${tree.label ?? tree.tree ?? "unnamed"} ${relation}`
      : mr ? `Your open MR ${mr.label?.match(/!\d+/)?.[0] ?? ""} mentions ${key}.`
      : "Assigned to you in Jira; nothing started yet.";
    if (!isClosedTicket(row)) return why;
    return tree || mr ? `The ticket is closed, but ${why.charAt(0).toLowerCase()}${why.slice(1)}`
      : "The ticket is closed; no worktree or open MR remains.";
  }
  if (row.kind === "mr") return "Your open MR has no ticket key in its branch or title.";
  if (row.kind === "tree") return "This worktree's branch matches no ticket or task.";
  return "This item is not attached to a task.";
}

export function stripFrontmatter(text) {
  const source = String(text ?? "");
  if (!/^---\r?\n/.test(source)) return source;
  return source.replace(/^---\r?\n[\s\S]*?^---\r?$(?:\n)?/m, "");
}

export function obsidianUri(path) {
  return "obsidian://open?path=" + encodeURIComponent(path);
}

export function cleanObsidian(body = "") {
  return String(body ?? "")
    .split("\n").filter(line => !/^\s*!\[\[[^\]]*\]\]\s*$/.test(line)).join("\n")
    .replace(/!\[\[[^\]]*\]\]/g, "")
    .replace(/\[\[([^\]]+)\]\]/g, (_match, link) => {
      const [target, alias] = link.split("|");
      return alias ?? target.split("#")[0];
    }).trim();
}

export function matchesFilter(row, query) {
  const needle = String(query ?? "").trim().toLowerCase();
  if (!needle) return true;
  return walk([row]).some(item => [item.title, item.label, item.task, item.target,
    item.branch, ...(item.tags ?? [])].filter(Boolean).join(" ").toLowerCase().includes(needle));
}

export function taskDetail(row, liveAgents = [], liveByTerminal = new Map(liveAgents.flatMap(agent => [[agent.address, agent], [agent.pane, agent]]))) {
  const children = row?.children ?? [];
  const tickets = children.filter(item => item.kind === "ticket");
  const agentDetail = item => {
    const live = item.terminal ? liveByTerminal.get(item.terminal) : undefined;
    return { row:item, live, status:live ? live.status : item.state ?? "unknown" };
  };
  const treeRank = tree => tree.agents.some(agent => agent.live) ? 0
    : tree.mr?.state?.toLowerCase() === "opened" ? 1
    : isMergedTree(tree.row) || ["merged", "closed"].includes(tree.mr?.state?.toLowerCase()) ? 3 : 2;
  const treeName = tree => tree.row.label ?? tree.row.tree ?? "";
  const trees = children.filter(item => item.kind === "tree").map(tree => ({
    row:tree,
    mr:tree.children?.find(item => item.kind === "mr") ?? null,
    agents:(tree.children ?? []).filter(item => item.kind === "agent").map(agentDetail),
  })).sort((a, b) => treeRank(a) - treeRank(b) || treeName(a).localeCompare(treeName(b)));
  const mrKey = mr => mr.url || mr.id || mr.target || mr.label || mr;
  const attached = new Set(trees.filter(tree => tree.mr).map(tree => mrKey(tree.mr)));
  const loose = new Map();
  const addMr = (mr, ticket) => {
    const key = mrKey(mr);
    if (attached.has(key)) return;
    if (!loose.has(key)) loose.set(key, {row:mr, tickets:[]});
    const tags = loose.get(key).tickets;
    if (ticket && !tags.includes(ticket)) tags.push(ticket);
  };
  for (const child of children) {
    if (child.kind === "mr") addMr(child);
    if (child.kind === "ticket") for (const mr of child.children ?? []) {
      if (mr.kind === "mr") addMr(mr, child.target);
    }
  }
  const paragraph = cleanObsidian(beforeMachineSection(row?.body)).split(/\n\s*\n/).map(plainParagraph).find(Boolean) ?? "";
  return {
    tickets:{open:tickets.filter(item => !isClosedTicket(item)), closed:tickets.filter(isClosedTicket)},
    trees,
    mainAgents:children.filter(item => item.kind === "agent").map(agentDetail),
    openMrs:[...loose.values()].filter(item => item.row.state?.toLowerCase() === "opened"),
    history:[...loose.values()].filter(item => ["merged", "closed"].includes(item.row.state?.toLowerCase())),
    subtasks:children.filter(item => item.kind === "task"),
    excerpt:paragraph.length > 240 ? `${paragraph.slice(0, 239).trimEnd()}…` : paragraph,
  };
}

function beforeMachineSection(body) {
  const lines = String(body ?? "").split(/\r?\n/);
  let fence = null;
  for (let index = 0; index < lines.length; index++) {
    const marker = lines[index].match(/^ {0,3}(`{3,}|~{3,})/);
    if (marker) {
      if (!fence) fence = marker[1];
      else if (marker[1][0] === fence[0] && marker[1].length >= fence.length &&
          !lines[index].slice(marker[0].length).trim()) fence = null;
    } else if (!fence && /^ {0,3}#{2,6}[ \t]+(?:agent notes|sessions|сессии)[ \t]*#*[ \t]*$/i.test(lines[index]))
      return lines.slice(0, index).join("\n");
  }
  return lines.join("\n");
}

function plainParagraph(text) {
  return text.replace(/^\s{0,3}#{1,6}\s+/gm, "")
    .replace(/^\s*(?:[-+*]|\d+[.)])\s+/gm, "")
    .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/(\*\*|__)(.*?)\1/g, "$2").replace(/\*([^*]+)\*/g, "$1")
    .replace(/(^|\W)_([^_]+)_(?=\W|$)/g, "$1$2").replace(/`+([^`]*?)`+/g, "$1")
    .replace(/\s+/g, " ").trim();
}

export function taskSignals(row, liveAgents = [], liveSpaces = [], related = walk(row.children),
  live = new Map(liveAgents.flatMap(agent => [[agent.address, agent], [agent.pane, agent]])), relatedFor = row => walk(row.children)) {
  const terminals = new Set(related.filter(item => item.kind === "agent").map(item => item.terminal).filter(Boolean));
  const taskAgents = [...new Set([...terminals].map(terminal => live.get(terminal)).filter(Boolean))];
  const working = taskAgents.filter(agent => agent.status === "working").length;
  const attention = taskAgents.some(agent => agent.status === "blocked") ? "blocked"
    : taskAgents.some(agent => agent.status === "done") ? "done" : null;
  const status = row.done ? "closed" : attention ?? (working ? "working" : row.state === "idea" ? "idea" : "open");
  const workingAgent = taskAgents.filter(agent => agent.status === "working")
    .reduce((latest, agent) => !latest || (agent.seq ?? 0) > (latest.seq ?? 0) ? agent : latest, null);
  const seen = new Set();
  const mrs = related.filter(item => {
    if (item.kind !== "mr" || item.state?.toLowerCase() !== "opened" || seen.has(item.id)) return false;
    seen.add(item.id);
    return true;
  });
  return {
    status, working, agents:taskAgents,
    workingAgent,
    workingTree:workingAgent && related.find(item => item.kind === "tree" && relatedFor(item).some(child =>
      child.kind === "agent" && child.terminal && [workingAgent.address, workingAgent.pane].includes(child.terminal))),
    attention: attention === "blocked" ? "Needs you" : attention === "done" ? "Finished"
      : working ? `${working} working` : "",
    ticket: related.find(item => item.kind === "ticket" && !isClosedTicket(item) && item.target),
    mrs, mrKey: mrs[0]?.label?.match(/!\d+/)?.[0] ?? "",
    mrFlag: parseMrFlags(mrs[0]?.body).worst,
    trees: related.filter(item => item.kind === "tree"),
    liveSpace: liveSpaces.find(space => space.id === row.space),
  };
}

export const sectionLabels = {
  needs:"Needs you", running:"Running", progress:"In progress", ideas:"Ideas",
  inbox:"Inbox", notInTask:"Not in a task", cleanup:"Cleanup", done:"Done",
};

export function groupSections(rows = [], liveAgents = [], liveSpaces = [], signalFor = row => taskSignals(row, liveAgents, liveSpaces)) {
  const sections = Object.entries(sectionLabels).map(([id, label]) => ({id, label, rows:[], collapsible:true}));
  const byId = new Map(sections.map(section => [section.id, section]));
  const seen = new Set();
  const tasks = (rows ?? []).flatMap(row => row.kind === "header" ? row.children ?? [] : [row]);
  for (const row of tasks) {
    if (row.kind !== "task" || seen.has(row.id)) continue;
    seen.add(row.id);
    const signal = signalFor(row);
    const section = row.done ? "done" : ["blocked", "done"].includes(signal.status) ? "needs"
      : signal.working ? "running" : row.state === "idea" ? "ideas" : "progress";
    byId.get(section).rows.push(row);
  }
  let looseSection = "cleanup";
  const looseIds = new Set();
  const addLoose = row => {
    if (row.kind === "task") return;
    if (row.kind === "header") {
      for (const child of row.children ?? []) addLoose(child);
    } else if (!looseIds.has(row.id)) {
      looseIds.add(row.id);
      const section = looseSection === "inbox" ? "inbox"
        : row.kind === "ticket" && !isClosedTicket(row) ? "notInTask" : "cleanup";
      byId.get(section).rows.push(row);
    }
  };
  // Snapshots may nest rows under a header or use it as a sibling delimiter.
  for (const row of rows ?? []) {
    if (row.kind === "header") {
      looseSection = String(row.label ?? "").trim().toLowerCase() === "assigned to me in jira"
        ? "inbox" : "cleanup";
      for (const child of row.children ?? []) addLoose(child);
    } else if (row.kind !== "task") addLoose(row);
  }
  return sections;
}

export function looseReason(row) {
  if (isMergedTree(row) || row.children?.some(isMergedTree)) return "merged · worktree left";
  if (row.kind === "ticket") {
    const tree = (row.children ?? []).find(child => child.kind === "tree" &&
      (!isClosedTicket(row) || child.state?.toLowerCase() === "active"));
    const mrs = [...new Map(walk(row.children)
      .filter(child => child.kind === "mr" && child.state?.toLowerCase() === "opened")
      .map(mr => [mr.url || mr.id || mr, mr])).values()];
    if (!isClosedTicket(row)) return tree ? `worktree ${tree.label ?? tree.tree ?? "unnamed"}`
      : mrs.length ? `MR ${mrs[0].label?.match(/!\d+/)?.[0] ?? mrs[0].target ?? ""} open` : "ticket open";
    const reason = tree ? "ticket closed · worktree left" : "ticket closed";
    return mrs.length ? `${reason} · ${mrs.length} ${mrs.length === 1 ? "MR" : "MRs"} open` : reason;
  }
  const age = row.kind === "mr" ? parseMrFlags(row.body).age : null;
  return age === null ? "no ticket" : `no ticket · ${age}d`;
}

export function visibleSections(sections, { query = "", showDone = false, collapsed = new Set() } = {}) {
  return sections.filter(section => section.id !== "done" || showDone).map(section => {
    const rows = section.rows.filter(row => matchesFilter(row, query));
    return { ...section, rows, expanded: Boolean(query.trim()) || !collapsed.has(section.id) };
  }).filter(section => section.rows.length);
}

export function visibleRows(sections) {
  return sections.flatMap(section => section.expanded ? section.rows : []);
}

export function ticketTitle(row) {
  const label = String(row.label ?? "").trim();
  return row.target && label.startsWith(row.target) ? label.slice(row.target.length).trim() : label;
}

export function rowState(row) {
  if (isMergedTree(row)) return "Merged";
  if (row.kind === "ticket") return normalizeTicketState(row.state);
  if (row.kind === "mr") return { opened: "Open", merged: "Merged", closed: "Closed" }[row.state?.toLowerCase()] ?? normalizeTicketState(row.state);
  if (row.kind === "tree" && row.state === "active") return "";
  return normalizeTicketState(row.state);
}

export function treeMarks(row) {
  const marks = parseTreeMarks(row?.body, row?.state);
  return {...marks, merged:row?.merged === true || row?.mr_state === "merged" || marks.merged ||
    row?.children?.some(child => child.kind === "mr" && child.state === "merged") === true};
}

export function isMergedTree(row) {
  return row?.kind === "tree" && treeMarks(row).merged;
}

export function mergedPlan(trees = []) {
  const remove = [], keep = [];
  for (const row of trees) {
    const marks = treeMarks(row);
    const reason = marks.changes ? "uncommitted changes" : marks.unpushed ? "unpushed commits"
      : row.mr_state === "opened" || row.children?.some(child => child.kind === "mr" && child.state === "opened")
        ? "open MR" : !marks.merged ? "not merged" : null;
    if (reason) keep.push({tree:row.tree ?? row.label, reason});
    else remove.push({tree:row.tree ?? row.label, path:row.cwd ?? marks.path,
      uncommitted:marks.changes, unpushed:marks.unpushed, merged:true});
  }
  return {kind:"trees", blockers:[], remove, keep, close_tickets:[], leave_tickets:[],
    note_done:false, close_space:false, steps:[]};
}

export function backendClosePlan(plan) {
  if (!plan || !["task", "trees"].includes(plan.kind) ||
      !["blockers", "remove", "keep", "close_tickets", "leave_tickets", "steps"].every(key => Array.isArray(plan[key])))
    throw new Error("Invalid close plan");
  return {blockers:plan.blockers, removals:plan.remove.map(tree => ({name:tree.tree, path:tree.path,
    changes:tree.uncommitted ?? 0, unpushed:tree.unpushed ?? 0})), keep:plan.keep,
    jira:plan.close_tickets, leave:plan.leave_tickets,
    also:[...(plan.note_done ? ["Marks the note done"] : []), ...(plan.close_space ? ["Closes the Herdr space"] : [])],
    proof:{count:0, latest:null}};
}

// Snapshot traversal belongs here, once per snapshot. Hover/focus redraws reuse it.
export class PanelModel {
  update(data, liveAgents = [], liveSpaces = [], options = {}) {
    const key = liveAgents.map(agent => `${agent.address}:${agent.status}:${agent.seq}:${agent.pane}`).join("|") + "/" +
      liveSpaces.map(space => `${space.id}:${space.active}:${(space.tabs ?? []).map(tab => `${tab.id}:${tab.working_since}`).join(",")}`).join("|");
    if (this.data !== data || !this.related) {
      this.data = data; this.related = new Map(); this.search = new Map();
      this.trees = []; const treeKeys = new Set();
      const visit = row => {
        const children = [];
        let search = [row.title, row.label, row.task, row.target, row.branch, ...(row.tags ?? [])].filter(Boolean).join(" ").toLowerCase();
        for (const child of row.children ?? []) {
          visit(child); children.push(child, ...this.related.get(child));
          search += " " + this.search.get(child);
        }
        this.related.set(row, children); this.search.set(row, search);
        if (row.kind === "tree") {
          const id = row.cwd || row.tree || row.id || row;
          if (!treeKeys.has(id)) { treeKeys.add(id); this.trees.push(row); }
        }
      };
      for (const row of data?.rows ?? []) visit(row);
      this.key = null;
    }
    if (this.key !== key) {
      this.activities = new Map();
      this.key = key; this.agents = liveAgents; this.spaces = liveSpaces;
      this.live = new Map(liveAgents.flatMap(agent => [[agent.address, agent], [agent.pane, agent]]));
      this.signals = new Map(); this.details = new Map(); this.reasons = new Map(); this.whys = new Map(); this.plans = new Map();
      this.sections = groupSections(data?.rows, liveAgents, liveSpaces, row => this.signal(row));
      this.mergedTrees = this.trees.filter(isMergedTree);
      this.bulkPlan = mergedPlan(this.trees);
      this.visibleKey = null;
    }
    const visibleKey = JSON.stringify([options.query ?? "", Boolean(options.showDone),
      [...(options.collapsed ?? [])].sort(), options.expanded ?? null]);
    if (this.visibleKey !== visibleKey) {
      this.visibleKey = visibleKey;
      const needle = String(options.query ?? "").trim().toLowerCase();
      this.visible = this.sections.filter(section => section.id !== "done" || options.showDone).map(section => ({...section,
        rows:section.rows.filter(row => !needle || this.search.get(row)?.includes(needle)),
        expanded:Boolean(needle) || !options.collapsed?.has(section.id),
      })).filter(section => section.rows.length);
      this.rows = visibleRows(this.visible);
    }
    return this;
  }
  signal(row) {
    if (!this.signals.has(row)) this.signals.set(row, taskSignals(row, this.agents, this.spaces, this.related.get(row) ?? walk(row.children), this.live, item => this.related.get(item) ?? walk(item.children)));
    return this.signals.get(row);
  }
  detail(row) {
    if (!this.details.has(row)) this.details.set(row, taskDetail(row, this.agents, this.live));
    return this.details.get(row);
  }
  reason(row) {
    if (!this.reasons.has(row)) this.reasons.set(row, looseReason(row));
    return this.reasons.get(row);
  }
  why(row) {
    if (!this.whys.has(row)) this.whys.set(row, looseWhy(row));
    return this.whys.get(row);
  }
  plan(row) {
    if (!this.plans.has(row)) this.plans.set(row, closePlan(row, this.spaces, this.related.get(row)));
    return this.plans.get(row);
  }
}

export function parseFrontmatter(text) {
  const source = String(text ?? "");
  if (!/^---\r?\n/.test(source)) return {properties:{}, body:source};
  const match = source.match(/^---\r?\n([\s\S]*?)^---\r?$(?:\n)?/m);
  if (!match) return {properties:{}, body:source};
  const properties = {};
  const scalar = value => {
    value = value.trim();
    if ((value.startsWith('"') && value.endsWith('"')) || (value.startsWith("'") && value.endsWith("'")))
      return value.slice(1, -1);
    return value.replace(/\s+#.*$/, "");
  };
  let list = null;
  for (const line of match[1].split(/\r?\n/)) {
    const entry = line.match(/^([\w-]+):\s*(.*)$/);
    if (entry) {
      const [, key, raw] = entry;
      list = raw ? null : key;
      properties[key] = raw.startsWith("[") && raw.endsWith("]")
        ? (raw.slice(1, -1).match(/"[^"]*"|'[^']*'|[^,]+/g) ?? []).map(scalar).filter(Boolean)
        : raw ? scalar(raw) : [];
    } else if (list) {
      const item = line.match(/^\s*-\s+(.+)$/);
      if (item && !/^[\w-]+:\s/.test(item[1])) properties[list].push(scalar(item[1]));
      else if (line.trim() && !line.trim().startsWith("#")) list = null;
    }
  }
  return {properties, body:source.slice(match[0].length)};
}

// Resolve only paths within the note directory, never arbitrary scheme targets.
export function wikiPath(path, target) {
  const name = String(target ?? "").split("#")[0].trim();
  if (!name || /^(?:\/|[a-z]+:)/i.test(name) || name.split(/[\\/]/).includes("..")) return null;
  const folder = path.slice(0, path.lastIndexOf("/")) || "/";
  return `${folder === "/" ? "" : folder}/${name}${name.endsWith(".md") ? "" : ".md"}`;
}

function outsideCode(text, transform) {
  return String(text ?? "").split(/(^\s*```[^\n]*\n[\s\S]*?^\s*```\s*$|^\s*~~~[^\n]*\n[\s\S]*?^\s*~~~\s*$|`[^`\n]*`)/gm)
    .map((part, index) => index % 2 ? part : transform(part)).join("");
}

export function wikiTargets(text) {
  const targets = new Set();
  outsideCode(text, part => {
    for (const match of part.matchAll(/(!?)\[\[([^\]]+)\]\]/g))
      if (!match[1]) targets.add(match[2].split("|")[0]);
    return part;
  });
  return [...targets];
}

export function transformWikilinks(text, path, existing = new Set()) {
  const escape = label => label.replace(/[\\[\]*_`<>]/g, "\\$&");
  return outsideCode(text, part => part.replace(/(!?)\[\[([^\]]+)\]\]/g, (_match, embed, link) => {
    const [target, alias] = link.split("|");
    const label = alias ?? target.split("#")[0];
    if (embed) return `![[${label}]]`; // The note renderer turns embeds into dim chips.
    const destination = wikiPath(path, target);
    return destination && existing.has(destination)
      ? `[${escape(label)}](berbr-note:${encodeURIComponent(destination)})` : escape(label);
  }));
}
