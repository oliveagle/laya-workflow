// dingtalk-ax — read the DingTalk macOS app by driving its UI through the
// Accessibility API. This is the "no public API" path: DingTalk exposes no way
// to pull chat history, but the running app renders messages as AX elements, so
// we can enumerate conversations and read the open conversation's messages.
//
//   dingtalk-ax sessions [--json]   list conversations (name / last message / time)
//   dingtalk-ax chat     [--json]   read the currently open conversation's messages
//   dingtalk-ax click N             open conversation #N (mouse click by AX geometry)
//   dingtalk-ax dump     [depth]    raw AX tree (debugging)
//
// Requires Accessibility permission for the process that runs it (System
// Settings → Privacy & Security → Accessibility). Read-only: it never types or
// sends. `click` only selects a conversation.

import Cocoa
import ApplicationServices
import CoreGraphics

func attr(_ e: AXUIElement, _ n: String) -> CFTypeRef? {
    var v: CFTypeRef?
    return AXUIElementCopyAttributeValue(e, n as CFString, &v) == .success ? v : nil
}
func s(_ e: AXUIElement, _ n: String) -> String? { attr(e, n) as? String }
func kids(_ e: AXUIElement) -> [AXUIElement] { (attr(e, kAXChildrenAttribute as String) as? [AXUIElement]) ?? [] }
func all(_ root: AXUIElement, role: String? = nil) -> [AXUIElement] {
    var out: [AXUIElement] = []; var st = [root]
    while let e = st.popLast() {
        if role == nil || s(e, kAXRoleAttribute as String) == role { out.append(e) }
        st.append(contentsOf: kids(e))
    }
    return out
}
func texts(_ e: AXUIElement) -> [(String, String)] {
    all(e, role: "AXStaticText").compactMap { c in
        guard let v = s(c, kAXValueAttribute as String), !v.isEmpty else { return nil }
        return (s(c, kAXIdentifierAttribute as String) ?? "", v)
    }
}
func point(_ e: AXUIElement, _ n: String) -> CGPoint? {
    guard let v = attr(e, n) else { return nil }
    var p = CGPoint.zero
    return AXValueGetValue(v as! AXValue, .cgPoint, &p) ? p : nil
}
func size(_ e: AXUIElement) -> CGSize? {
    guard let v = attr(e, kAXSizeAttribute as String) else { return nil }
    var sz = CGSize.zero
    return AXValueGetValue(v as! AXValue, .cgSize, &sz) ? sz : nil
}
func click(_ p: CGPoint) {
    let src = CGEventSource(stateID: .combinedSessionState)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(60_000)
    CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(40_000)
    CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
}
func jstr(_ x: String) -> String {
    var o = "\""
    for c in x.unicodeScalars {
        switch c {
        case "\"": o += "\\\""
        case "\\": o += "\\\\"
        case "\n": o += "\\n"
        case "\r": o += "\\r"
        case "\t": o += "\\t"
        default: o.unicodeScalars.append(c.value < 0x20 ? "." : c)
        }
    }
    return o + "\""
}

let TAG = ["老师", "班主任", "任课老师", "管理员", "群主"]
var args = Array(CommandLine.arguments.dropFirst())
let json = args.contains("--json")
args.removeAll { $0 == "--json" }
let cmd = args.first ?? "sessions"

guard let app = NSWorkspace.shared.runningApplications.first(where: { ($0.bundleIdentifier ?? "").lowercased().contains("dingtalk") }) else {
    FileHandle.standardError.write("DingTalk is not running\n".data(using: .utf8)!); exit(1)
}
let ax = AXUIElementCreateApplication(app.processIdentifier)

func sessionNameEls() -> [AXUIElement] {
    all(ax, role: "AXStaticText").filter { s($0, kAXIdentifierAttribute as String) == "_NS:197" }
}
func sessions() -> [(String, String, String)] {
    sessionNameEls().map { e in
        var prev = "", time = ""
        if let p = attr(e, kAXParentAttribute as String) as! AXUIElement? {
            for (id, v) in texts(p) { if id == "_NS:87" { prev = v }; if id == "_NS:151" { time = v } }
        }
        return (s(e, kAXValueAttribute as String) ?? "?", prev, time)
    }
}
func messages() -> [(String, String, String, [String])] {
    for tbl in all(ax, role: "AXTable") {
        let rows = kids(tbl).filter { s($0, kAXRoleAttribute as String) == "AXRow" }
        if rows.isEmpty { continue }
        return rows.map { r in
            let sender = all(r, role: "AXButton").compactMap { s($0, kAXTitleAttribute as String) }
                .first { !$0.isEmpty && !TAG.contains($0) && $0 != "点此预览" && !$0.hasPrefix("(") } ?? ""
            let txt = all(r, role: "AXTextArea").compactMap { s($0, kAXValueAttribute as String) }.first { !$0.isEmpty } ?? ""
            let time = texts(r).map { $0.1 }.first { $0.contains(":") || $0.contains("-") } ?? ""
            let files = texts(r).filter { $0.1.hasSuffix("KB") || $0.1.hasSuffix("MB") }.map { $0.1 }
            return (sender, txt, time, files)
        }
    }
    return []
}

switch cmd {
case "sessions":
    let rows = sessions()
    if json {
        print("[" + rows.map { "{\"name\":\(jstr($0.0)),\"preview\":\(jstr($0.1)),\"time\":\(jstr($0.2))}" }.joined(separator: ",") + "]")
    } else {
        print("# conversations (\(rows.count))")
        for (i, r) in rows.enumerated() {
            print(String(format: "%2d. %@  [%@]", i, r.0, r.2))
            print("     " + String(r.1.replacingOccurrences(of: "\n", with: " / ").prefix(120)))
        }
    }
case "chat":
    let rows = messages()
    if json {
        print("[" + rows.map { m in
            "{\"sender\":\(jstr(m.0)),\"text\":\(jstr(m.1)),\"time\":\(jstr(m.2)),\"files\":[" + m.3.map(jstr).joined(separator: ",") + "]}"
        }.joined(separator: ",") + "]")
    } else {
        print("# messages (\(rows.count))")
        for m in rows {
            var body = m.1.isEmpty ? (m.3.isEmpty ? "—" : "[file] " + m.3.joined(separator: ", ")) : m.1
            body = body.replacingOccurrences(of: "\n", with: " / ")
            print("[\(m.2.isEmpty ? "??" : m.2)] \(m.0.isEmpty ? "?" : m.0): \(body)")
        }
    }
case "click":
    guard let i = Int(args.dropFirst().first ?? ""), i >= 0 else { print("usage: click <idx>"); exit(2) }
    let els = sessionNameEls()
    guard i < els.count else { print("out of range (\(els.count))"); exit(2) }
    guard let grp = attr(els[i], kAXParentAttribute as String) as! AXUIElement?,
          let p = point(grp, kAXPositionAttribute as String), let sz = size(grp) else { print("no geometry"); exit(3) }
    let t = CGPoint(x: p.x + sz.width - 40, y: p.y + sz.height / 2)
    FileHandle.standardError.write("opening '\(s(els[i], kAXValueAttribute as String) ?? "?")' at \(t)\n".data(using: .utf8)!)
    app.activate(options: [])
    Thread.sleep(forTimeInterval: 0.6)
    click(t)
    Thread.sleep(forTimeInterval: 1.5)
    print("ok")
case "dump":
    let maxD = Int(args.dropFirst().first ?? "") ?? 6
    var lines: [String] = []
    func walk(_ e: AXUIElement, _ d: Int) {
        if d > maxD { return }
        var bits = [s(e, kAXRoleAttribute as String) ?? "?"]
        for k in [kAXTitleAttribute, kAXValueAttribute, kAXIdentifierAttribute] {
            if let v = s(e, k as String), !v.isEmpty { bits.append("\(k)=\(v)") }
        }
        lines.append(String(repeating: "  ", count: d) + bits.joined(separator: " | "))
        for k in kids(e) { walk(k, d + 1) }
    }
    walk(ax, 0)
    print(lines.joined(separator: "\n"))
default:
    print("commands: sessions | chat | click <idx> | dump [depth]   (--json for JSON)")
}
