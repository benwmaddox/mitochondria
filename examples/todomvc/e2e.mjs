// TodoMVC's app spec (todomvc.com/app-spec), checked in a browser against the running app: the same behaviors as crudspec's
// apps/todomvc/e2e.mjs (and the reference implementations' tests), through what a person does. A second pass turns script off
// and checks the pages and forms alone.
//   node examples/todomvc/e2e.mjs <base-url> [screenshot.png]
// PLAYWRIGHT: a folder whose node_modules has playwright (default D:/code/crudspec); CHROME: a Chromium (default Playwright's own)
import { createRequire } from "node:module";
const require = createRequire((process.env.PLAYWRIGHT || "D:/code/crudspec") + "/x.js");
const { chromium } = require("playwright");
const exe = process.env.CHROME || undefined;
const [base, shot] = process.argv.slice(2);
let failed = 0;
const ok = (what, pass, detail) => { console.log(`${pass ? "ok  " : "FAIL"} ${what}`); if (!pass) { failed++; if (detail !== undefined) console.log("     " + JSON.stringify(detail)); } };
const b = await chromium.launch(exe ? { executablePath: exe } : {});

async function spec(page, label, scripted) {
  const settle = () => page.waitForLoadState("networkidle").catch(() => {});
  const nav = async (f) => { await Promise.all([page.waitForNavigation().catch(() => {}), f()]); await settle(); };
  const input = () => page.getByPlaceholder("What needs to be done?");
  const add = async (t) => { await input().fill(t); await nav(() => input().press("Enter")); };
  const titles = async () => page.locator(".todo-list li label").allInnerTexts().then((l) => l.map((x) => x.trim()));
  const item = (t) => page.locator(".todo-list li").filter({ hasText: t });
  const count = async () => (await page.locator(".todo-count").innerText().catch(() => "")).trim().replace(/\s+/g, " ");
  const toggle = (t) => nav(() => scripted ? item(t).locator(".toggle").click() : item(t).getByRole("button", { name: "Done" }).click());
  const p = (s) => `${label}: ${s}`;
  await page.goto(base + "/"); await settle();
  ok(p("no todos: no main section and no footer"), (await page.locator(".main").count()) === 0 && (await page.locator(".footer").count()) === 0);
  ok(p("the new-todo field has the focus"), await input().evaluate((e) => e === document.activeElement));
  await add("   buy milk   ");
  ok(p("a new todo is added, trimmed"), JSON.stringify(await titles()) === '["buy milk"]', await titles());
  ok(p("the input is cleared after adding"), (await input().inputValue()) === "");
  await add("   ");
  ok(p("an empty todo isn't added"), (await titles()).length === 1);
  await add("walk the dog"); await add("water plants");
  ok(p("todos are listed in the order added"), JSON.stringify(await titles()) === '["buy milk","walk the dog","water plants"]', await titles());
  ok(p("the counter says 3 items left"), (await count()) === "3 items left", await count());
  await toggle("walk the dog");
  ok(p("a todo can be marked complete"), (await item("walk the dog").getAttribute("class")).includes("completed"));
  ok(p("the counter follows (2 items left)"), (await count()) === "2 items left", await count());
  ok(p("Clear completed shows when a todo is complete"), (await page.getByRole("button", { name: "Clear completed" }).count()) === 1);
  await nav(() => page.getByRole("link", { name: "Active" }).click());
  ok(p("the Active filter shows the active ones"), JSON.stringify(await titles()) === '["buy milk","water plants"]', await titles());
  ok(p("and is selected"), (await page.getByRole("link", { name: "Active" }).getAttribute("class")).includes("selected"));
  await nav(() => page.getByRole("link", { name: "Completed" }).click());
  ok(p("the Completed filter shows the completed ones"), JSON.stringify(await titles()) === '["walk the dog"]', await titles());
  await nav(() => page.getByRole("link", { name: "All" }).click());
  return { settle, nav, input, add, titles, item, count, toggle, p };
}

async function editing(page, h) {
  const { nav, titles, item, p } = h;
  const edit = () => page.locator(".todo-list li.editing .edit");
  await item("buy milk").locator("label").dblclick();
  ok(p("double-clicking a todo opens it for editing"), (await edit().count()) === 1 && (await edit().inputValue()) === "buy milk");
  ok(p("the edit field has the focus"), await edit().evaluate((e) => e === document.activeElement));
  await edit().fill("  buy oat milk  "); await nav(() => edit().press("Enter"));
  ok(p("Enter saves the edit, trimmed"), (await titles()).includes("buy oat milk") && (await page.locator(".todo-list li.editing").count()) === 0, await titles());
  await item("water plants").locator("label").dblclick();
  await edit().fill("something else"); await edit().press("Escape"); await page.waitForTimeout(300);
  ok(p("Escape cancels the edit"), (await titles()).includes("water plants") && !(await titles()).includes("something else") && (await page.locator(".todo-list li.editing").count()) === 0, await titles());
  await item("water plants").locator("label").dblclick();
  await edit().fill("water all plants"); await nav(() => page.locator("h1").click());
  ok(p("leaving the field saves the edit"), (await titles()).includes("water all plants"), await titles());
  await item("buy oat milk").locator("label").dblclick();
  await edit().fill(""); await nav(() => edit().press("Enter"));
  ok(p("an edit left empty removes the todo"), !(await titles()).includes("buy oat milk"), await titles());
}

async function rest(page, h, scripted) {
  const { nav, titles, item, count, toggle, add, settle, p } = h;
  const markAll = () => scripted ? page.locator("label[for=toggle-all]").click() : page.getByRole("button", { name: "Mark all" }).click();
  await nav(markAll);
  ok(p("Mark all as complete completes them all"), (await page.locator(".todo-list li.completed").count()) === (await page.locator(".todo-list li").count()));
  ok(p("the counter says 0 items left"), (await count()) === "0 items left", await count());
  await nav(markAll);
  ok(p("again, it makes them all active"), (await page.locator(".todo-list li.completed").count()) === 0);
  await toggle("walk the dog");
  // with script the list has two todos here (one was removed by an empty edit), without it three
  ok(p(scripted ? "1 item left (singular)" : "2 items left"), (await count()) === (scripted ? "1 item left" : "2 items left"), await count());
  await nav(() => page.getByRole("button", { name: "Clear completed" }).click());
  ok(p("Clear completed removes the completed ones"), JSON.stringify(await titles()) === (scripted ? '["water all plants"]' : '["buy oat milk","water plants"]'), await titles());
  ok(p("and hides itself"), (await page.getByRole("button", { name: "Clear completed" }).count()) === 0);
  for (const t of await titles()) { await item(t).hover(); await nav(() => item(t).getByRole("button", { name: "Delete" }).click()); }
  ok(p("the destroy button removes a todo; the footer goes with the last one"), (await page.locator(".todo-list li").count()) === 0 && (await page.locator(".footer").count()) === 0);
  await add("persist me"); await page.reload(); await settle();
  ok(p("todos persist across reloads"), (await titles()).includes("persist me"));
  const other = await (await b.newContext()).newPage(); await other.goto(base + "/"); await other.waitForLoadState("networkidle");
  ok(p("another browser has its own list"), (await other.locator(".todo-list li").count()) === 0);
}

try {
  // with the script, as a person uses it
  const page = await (await b.newContext({ viewport: { width: 800, height: 900 } })).newPage();
  const errors = []; page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  const h = await spec(page, "script", true);
  await editing(page, h);
  await rest(page, h, true);
  await h.add("second item"); await h.toggle("second item");
  if (shot) await page.screenshot({ path: shot });
  ok("script: no errors in the console (a Content-Security-Policy refusal would show here)", errors.length === 0, errors);
  // without script: the same through pages and forms (editing through the Edit link)
  const plain = await (await b.newContext({ javaScriptEnabled: false })).newPage();
  const g = await spec(plain, "no script", false);
  await g.nav(() => g.item("buy milk").getByRole("link", { name: "Edit" }).click());
  const edit = plain.locator(".todo-list li.editing .edit");
  ok("no script: the Edit link opens a todo for editing", (await edit.count()) === 1 && (await edit.inputValue()) === "buy milk");
  await edit.fill("buy oat milk"); await g.nav(() => edit.press("Enter"));
  ok("no script: Enter saves it", (await g.titles()).includes("buy oat milk"), await g.titles());
  await rest(plain, g, false);
} catch (e) { ok("crashed", false, e.message); }
await b.close();
console.log(failed ? `${failed} failed` : "all passed");
process.exit(failed ? 1 : 0);
