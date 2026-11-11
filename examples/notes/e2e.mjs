// Drives a running notes server over HTTP (no browser): node examples/notes/e2e.mjs http://127.0.0.1:4311
// Each "browser" is a cookie jar; forms are posted url-encoded, redirects are read, not followed.
const base = process.argv[2] ?? "http://127.0.0.1:4311";
let failed = 0, passed = 0;
function check(name, ok, detail = "") {
  if (ok) passed++; else { failed++; console.log("FAIL", name, detail); }
}

class Browser {
  jar = "";
  async go(method, path, form) {
    const headers = { Cookie: this.jar, Origin: base };
    let body;
    if (form) { headers["Content-Type"] = "application/x-www-form-urlencoded"; body = new URLSearchParams(form).toString(); }
    const r = await fetch(base + path, { method, headers, body, redirect: "manual" });
    for (const c of r.headers.getSetCookie()) {
      const [kv] = c.split(";");
      const [k, v] = kv.split("=");
      const rest = this.jar.split("; ").filter((p) => p && !p.startsWith(k + "="));
      this.jar = (v ? [...rest, kv] : rest).join("; ");
    }
    return { status: r.status, location: r.headers.get("location"), text: await r.text() };
  }
  get = (p) => this.go("GET", p);
  post = (p, f) => this.go("POST", p, f);
  // the page the browser ends on after a form post
  async submit(p, f) { const r = await this.post(p, f); return r.location ? this.get(r.location) : r; }
}

const tag = Date.now().toString(36);
const ann = new Browser(), bo = new Browser(), anon = new Browser();

// signed out
let r = await anon.get("/");
check("home redirects to sign in when signed out", r.status === 303 && r.location === "/signin", JSON.stringify(r));
r = await anon.post("/notes", { title: "x" });
check("a note cannot be added without signing in", r.status >= 400, String(r.status));

// sign up, duplicates, wrong password
r = await ann.post("/signup", { email: `ann-${tag}@x.org`, password: "correct horse" });
check("sign up redirects home", r.status === 303 && r.location === "/", JSON.stringify(r));
r = await ann.get("/");
check("signed-in home shows the email and a form", r.status === 200 && r.text.includes(`ann-${tag}@x.org`) && r.text.includes('action="/notes"'));
r = await new Browser().post("/signup", { email: `ANN-${tag}@x.org`, password: "another pass" });
check("the same email in other case is taken", r.location === "/signin?error=taken", JSON.stringify(r));
r = await new Browser().post("/signup", { email: `short-${tag}@x.org`, password: "short" });
check("a short password is refused", r.status >= 400, String(r.status));
r = await new Browser().post("/signin", { email: `ann-${tag}@x.org`, password: "wrong password" });
check("a wrong password is refused", r.location === "/signin?error=wrong", JSON.stringify(r));
const again = new Browser();
r = await again.post("/signin", { email: `Ann-${tag}@x.org`, password: "correct horse" });
check("signing in works (email case ignored)", r.location === "/" && again.jar.includes("me="), JSON.stringify(r));

// notes
r = await ann.post("/notes", { title: "Groceries", body: "milk <b>& eggs</b>", tags: "Home  Shopping" });
check("adding a note redirects to it", /^\/notes\/\d+$/.test(r.location ?? ""), JSON.stringify(r));
const id = r.location.split("/").pop();
r = await ann.get(r.location);
check("the note is open for editing, text escaped", r.text.includes('value="Groceries"') && r.text.includes("milk &lt;b&gt;&amp; eggs&lt;/b&gt;") && !r.text.includes("<b>&"), r.text.slice(0, 400));
await ann.post("/notes", { title: "Plan the trip", body: "", tags: "travel" });
r = await ann.get("/");
check("both notes are listed, newest first", r.text.indexOf("Plan the trip") > 0 && r.text.indexOf("Plan the trip") < r.text.indexOf("Groceries"));
r = await ann.get("/?q=groc");
check("search by part of a title", r.text.includes("Groceries") && !r.text.includes("Plan the trip"));
r = await ann.get("/?tag=travel");
check("filter by tag", r.text.includes("Plan the trip") && !r.text.includes("Groceries"));
r = await ann.get("/?tag=shopping");
check("tags are lower case", r.text.includes("Groceries"));
r = await ann.submit(`/notes/${id}`, { title: "Groceries (done)", body: "", tags: "home" });
check("saving changes the note", r.text.includes('value="Groceries (done)"'));
r = await ann.post("/notes", { title: "   ", body: "", tags: "" });
r = await ann.get(r.location);
check("an empty title becomes Untitled", r.text.includes('value="Untitled"'));

// another user cannot see or touch them
r = await bo.post("/signup", { email: `bo-${tag}@x.org`, password: "battery staple" });
r = await bo.get("/");
check("another user's list is empty", r.text.includes("No notes yet") && !r.text.includes("Groceries"));
r = await bo.get(`/notes/${id}`);
check("another user cannot open a note by its number", !r.text.includes("Groceries"), String(r.status));
r = await bo.post(`/notes/${id}/delete`, {});
r = await ann.get(`/notes/${id}`);
check("another user cannot delete it", r.text.includes("Groceries (done)"), String(r.status));

// delete, sign out
r = await ann.post(`/notes/${id}/delete`, {});
r = await ann.get("/");
check("a deleted note is gone", !r.text.includes("Groceries (done)"));
const old = ann.jar;
r = await ann.post("/signout", {});
check("signing out goes to the sign-in page", r.location === "/signin" && !ann.jar.includes("me="), JSON.stringify(r));
const stale = new Browser(); stale.jar = old;
r = await stale.get("/");
check("the old cookie no longer works after signing out", r.location === "/signin", JSON.stringify(r));

console.log(`${passed} passed, ${failed} failed`);
process.exit(failed ? 1 : 0);
