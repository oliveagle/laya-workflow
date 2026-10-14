// A single-file Laya plugin: no directory, no plugin.json.
//
// A capability that points at this file runs it directly:
//   { "kind": "plugin", "dir": "dsl/capabilities/hello.rhai" }   // or "entry"
// A relative path like that resolves against the dsl root, the checkout
// that holds it, and the cwd, so the spec runs from anywhere.
// The default entry op is `run`; the plugin's name is this file's stem ("hello").
fn run(host, ctx) {
    let args = ctx["with"];
    let who = "world";
    if args.contains("who") && type_of(args["who"]) != "()" { who = "" + args["who"]; }
    #{ hi: "hello " + who, plugin: ctx["plugin"], op: ctx["op"], at: host.now()["rfc3339"] }
}
