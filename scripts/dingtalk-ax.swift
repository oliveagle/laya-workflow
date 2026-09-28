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
func rightClick(_ p: CGPoint) {
    let src = CGEventSource(stateID: .combinedSessionState)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .right)?.post(tap: .cghidEventTap)
    usleep(80_000)
    CGEvent(mouseEventSource: src, mouseType: .rightMouseDown, mouseCursorPosition: p, mouseButton: .right)?.post(tap: .cghidEventTap)
    usleep(60_000)
    CGEvent(mouseEventSource: src, mouseType: .rightMouseUp, mouseCursorPosition: p, mouseButton: .right)?.post(tap: .cghidEventTap)
}
func messageRows() -> [AXUIElement] {
    guard let tbl = messageTable() else { return [] }
    return kids(tbl).filter { s($0, kAXRoleAttribute as String) == "AXRow" }
}
func scroll(_ p: CGPoint, _ lines: Int32) {
    let src = CGEventSource(stateID: .combinedSessionState)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left)?.post(tap: .cghidEventTap)
    usleep(50_000)
    let e = CGEvent(scrollWheelEvent2Source: src, units: .line, wheelCount: 1, wheel1: lines, wheel2: 0, wheel3: 0)
    e?.location = p
    e?.post(tap: .cghidEventTap)
    usleep(80_000)
}
func scrollTo(_ pid: pid_t, _ p: CGPoint, _ lines: Int32) {
    let src = CGEventSource(stateID: .hidSystemState)
    if let m = CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: p, mouseButton: .left) { m.postToPid(pid) }
    usleep(40_000)
    if let e = CGEvent(scrollWheelEvent2Source: src, units: .line, wheelCount: 1, wheel1: lines, wheel2: 0, wheel3: 0) {
        e.location = p
        e.postToPid(pid)
    }
    usleep(120_000)
}
func messageTable() -> AXUIElement? {
    for tbl in all(ax, role: "AXTable") {
        if !kids(tbl).filter({ s($0, kAXRoleAttribute as String) == "AXRow" }).isEmpty { return tbl }
    }
    return nil
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
case "open":
    guard let needle = args.dropFirst().first, !needle.isEmpty else { print("usage: open <name-substring>"); exit(2) }
    let els = sessionNameEls()
    guard let idx = els.firstIndex(where: { (s($0, kAXValueAttribute as String) ?? "").contains(needle) }) else {
        print("no conversation matching '\(needle)'"); exit(2)
    }
    guard let grp = attr(els[idx], kAXParentAttribute as String) as! AXUIElement?,
          let p = point(grp, kAXPositionAttribute as String), let sz = size(grp) else { print("no geometry"); exit(3) }
    let t = CGPoint(x: p.x + sz.width - 40, y: p.y + sz.height / 2)
    FileHandle.standardError.write("opening '\(s(els[idx], kAXValueAttribute as String) ?? "?")' at \(t)\n".data(using: .utf8)!)
    app.activate(options: [])
    Thread.sleep(forTimeInterval: 0.6)
    click(t)
    Thread.sleep(forTimeInterval: 2.0)
    print("ok")
case "scroll":
    let dir = args.dropFirst().first ?? "up"
    let n = Int(args.dropFirst(2).first ?? "5") ?? 5
    guard let win = all(ax, role: "AXWindow").first,
          let p0 = point(win, kAXPositionAttribute as String), let sz = size(win) else { print("no window"); exit(3) }
    let c = CGPoint(x: p0.x + sz.width * 0.72, y: p0.y + sz.height * 0.5)
    FileHandle.standardError.write("window at \(p0) size \(sz) -> scroll at \(c)\n".data(using: .utf8)!)
    app.activate(options: [])
    Thread.sleep(forTimeInterval: 0.4)
    let lines: Int32 = dir == "down" ? Int32(-n) : Int32(n)
    for _ in 0..<n { scrollTo(app.processIdentifier, c, lines > 0 ? 3 : -3) }
    Thread.sleep(forTimeInterval: 0.8)
    print("scrolled \(dir) \(n) at \(c)")
case "key":
    let code = UInt16(args.dropFirst().first ?? "116") ?? 116
    let n = Int(args.dropFirst(2).first ?? "1") ?? 1
    app.activate(options: [.activateIgnoringOtherApps, .activateAllWindows])
    Thread.sleep(forTimeInterval: 0.5)
    let src = CGEventSource(stateID: .hidSystemState)
    for _ in 0..<n {
        if let d = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: true) { d.postToPid(app.processIdentifier) }
        if let u = CGEvent(keyboardEventSource: src, virtualKey: code, keyDown: false) { u.postToPid(app.processIdentifier) }
        usleep(250_000)
    }
    Thread.sleep(forTimeInterval: 0.6)
    print("sent key \(code) x\(n)")
case "sbar":
    app.activate(options: [.activateIgnoringOtherApps, .activateAllWindows])
    Thread.sleep(forTimeInterval: 0.4)
    var i = 0
    for sb in all(ax, role: "AXScrollBar") {
        var settable: DarwinBoolean = false
        AXUIElementIsAttributeSettable(sb, kAXValueAttribute as CFString, &settable)
        var orient = ""
        if let v = attr(sb, kAXOrientationAttribute as String) as? String { orient = v }
        let pos = point(sb, kAXPositionAttribute as String) ?? .zero
        let val = attr(sb, kAXValueAttribute as String)
        print("[\(i)] orient=\(orient) value=\(String(describing: val)) settable=\(settable.boolValue) pos=\(pos) id=\(s(sb, kAXIdentifierAttribute as String) ?? "")")
        i += 1
    }
    if let target = args.dropFirst().first, let v = Double(args.dropFirst(2).first ?? ""), let idx = Int(target) {
        let sbs = all(ax, role: "AXScrollBar")
        if idx < sbs.count {
            let r = AXUIElementSetAttributeValue(sbs[idx], kAXValueAttribute as CFString, v as CFTypeRef)
            print("set [\(idx)] -> \(r.rawValue)")
            Thread.sleep(forTimeInterval: 1.0)
        }
    }
case "clickat":
    let fx = Double(args.dropFirst().first ?? "0.95") ?? 0.95
    let fy = Double(args.dropFirst(2).first ?? "0.5") ?? 0.5
    guard let win = all(ax, role: "AXWindow").first,
          let p0 = point(win, kAXPositionAttribute as String), let sz = size(win) else { print("no window"); exit(3) }
    let c = CGPoint(x: p0.x + sz.width * fx, y: p0.y + sz.height * fy)
    app.activate(options: [.activateAllWindows])
    Thread.sleep(forTimeInterval: 0.5)
    click(c)
    Thread.sleep(forTimeInterval: 0.4)
    print("clicked at \(c)")
case "pageup", "pagedown":
    let n = Int(args.dropFirst().first ?? "5") ?? 5
    let act = (cmd == "pageup" ? "AXScrollUpByPage" : "AXScrollDownByPage") as CFString
    guard let tbl = messageTable(), let area = attr(tbl, kAXParentAttribute as String) as! AXUIElement? else { print("no message area"); exit(3) }
    for _ in 0..<n {
        let r = AXUIElementPerformAction(area, act)
        if r != .success { print("action rc=\(r.rawValue)"); break }
        Thread.sleep(forTimeInterval: 0.35)
    }
    Thread.sleep(forTimeInterval: 1.0)
    print("\(cmd) x\(n)")
case "wheel":
    let dir = args.dropFirst().first ?? "up"
    let n = Int(args.dropFirst(2).first ?? "10") ?? 10
    guard let win = all(ax, role: "AXWindow").first,
          let p0 = point(win, kAXPositionAttribute as String), let sz = size(win) else { print("no window"); exit(3) }
    let c = CGPoint(x: p0.x + sz.width * 0.72, y: p0.y + sz.height * 0.5)
    app.activate(options: [.activateAllWindows])
    Thread.sleep(forTimeInterval: 0.8)
    for _ in 0..<n { scroll(c, dir == "down" ? -2 : 2) }
    Thread.sleep(forTimeInterval: 1.0)
    print("wheel \(dir) \(n) at \(c)")
case "collect":
    // Scroll the message list to the top repeatedly, accumulating every row we
    // see. DingTalk loads older history when the top is reached; we stop when a
    // page adds nothing new. Output: a JSON array of row records.
    let maxPages = Int(args.dropFirst().first ?? "80") ?? 80
    guard let tbl0 = messageTable(),
          let area = attr(tbl0, kAXParentAttribute as String) as! AXUIElement?,
          let bar = all(area, role: "AXScrollBar").first(where: {
              (attr($0, kAXOrientationAttribute as String) as? String) == "AXVerticalOrientation"
          }) else { print("[]"); exit(3) }
    var seen = Set<String>()
    var out: [String] = []
    var stagnant = 0
    func rec(_ r: AXUIElement) -> String {
        let btns = all(r, role: "AXButton").compactMap { s($0, kAXTitleAttribute as String) }.filter { !$0.isEmpty }
        let ta = all(r, role: "AXTextArea").compactMap { s($0, kAXValueAttribute as String) }.filter { !$0.isEmpty }
        let st = texts(r)
        let imgN = all(r, role: "AXImage").count
        let time = st.map { $0.1 }.first { $0.contains(":") || $0.contains("-") || $0.contains("月") } ?? ""
        let sender = btns.first { !TAG.contains($0) && $0 != "点此预览" && !$0.hasPrefix("(") && !$0.hasPrefix("添加") && $0 != "下载" } ?? ""
        let files = st.map { $0.1 }.filter { $0.hasSuffix("KB") || $0.hasSuffix("MB") }
        let fnames = st.map { $0.1 }.filter { $0.contains(".pdf") || $0.contains(".doc") || $0.contains(".xls") || $0.contains(".zip") || $0.contains(".jpg") || $0.contains(".png") || $0.contains(".mp4") }
        var body = ta.first ?? ""
        if body.isEmpty { body = fnames.first.map { "[file] " + $0 } ?? "" }
        let text = ta.joined(separator: "\n")
        let key = sender + "|" + time + "|" + text + "|" + files.joined(separator: ",") + "|" + fnames.joined(separator: ",")
        var o = "{\"sender\":" + jstr(sender) + ",\"time\":" + jstr(time)
        o += ",\"text\":" + jstr(text)
        o += ",\"body\":" + jstr(body)
        o += ",\"files\":[" + files.map(jstr).joined(separator: ",") + "]"
        o += ",\"fnames\":[" + fnames.map(jstr).joined(separator: ",") + "]"
        o += ",\"buttons\":[" + btns.map(jstr).joined(separator: ",") + "]"
        o += ",\"imgs\":" + String(imgN) + "}"
        return key + "\u{0}" + o
    }
    for _ in 0..<maxPages {
        _ = AXUIElementSetAttributeValue(bar, kAXValueAttribute as CFString, 0.0 as CFTypeRef)
        Thread.sleep(forTimeInterval: 1.2)
        let rows = kids(tbl0).filter { s($0, kAXRoleAttribute as String) == "AXRow" }
        var added = 0
        for r in rows {
            let pair = rec(r)
            let parts = pair.split(separator: "\u{0}", maxSplits: 1)
            let key = String(parts[0])
            if seen.contains(key) { continue }
            seen.insert(key); out.append(String(parts[1])); added += 1
        }
        FileHandle.standardError.write("page: \\(rows.count) rows, +\\(added) new, total \\(out.count)\n".data(using: .utf8)!)
        if added == 0 { stagnant += 1; if stagnant >= 2 { break } } else { stagnant = 0 }
    }
    print("[" + out.joined(separator: ",") + "]")
case "scrollfrac":
    guard let f = Double(args.dropFirst().first ?? "0") else { print("usage: scrollfrac <0..1>"); exit(2) }
    guard let tbl = messageTable(), let area = attr(tbl, kAXParentAttribute as String) as! AXUIElement? else { print("no message area"); exit(3) }
    guard let bar = all(area, role: "AXScrollBar").first(where: {
        (attr($0, kAXOrientationAttribute as String) as? String) == "AXVerticalOrientation"
    }) else { print("no vertical scrollbar in message area"); exit(3) }
    let r = AXUIElementSetAttributeValue(bar, kAXValueAttribute as CFString, f as CFTypeRef)
    Thread.sleep(forTimeInterval: 1.2)
    print("set message scrollbar -> \(f) (rc=\(r.rawValue)); now \(String(describing: attr(bar, kAXValueAttribute as String)))")
case "diag":
    guard let tbl = messageTable() else { print("no table"); exit(3) }
    print("TABLE pos=\(String(describing: point(tbl, kAXPositionAttribute as String))) size=\(String(describing: size(tbl))) id=\(s(tbl, kAXIdentifierAttribute as String) ?? "") rows=\(kids(tbl).count)")
    var e: AXUIElement = tbl
    for lvl in 0..<7 {
        guard let par = attr(e, kAXParentAttribute as String) as! AXUIElement? else { break }
        e = par
        let bars = all(e, role: "AXScrollBar").map { b -> String in
            let o = (attr(b, kAXOrientationAttribute as String) as? String) ?? "?"
            let v = String(describing: attr(b, kAXValueAttribute as String))
            return "\(o)=\(v)@\(String(describing: point(b, kAXPositionAttribute as String)))"
        }
        print("up\(lvl): role=\(s(e, kAXRoleAttribute as String) ?? "?") id=\(s(e, kAXIdentifierAttribute as String) ?? "") pos=\(String(describing: point(e, kAXPositionAttribute as String))) size=\(String(describing: size(e))) bars=\(bars)")
    }
case "rows":
    for tbl in all(ax, role: "AXTable") {
        let rows = kids(tbl).filter { s($0, kAXRoleAttribute as String) == "AXRow" }
        if rows.isEmpty { continue }
        print("# rows (\(rows.count))")
        for (i, r) in rows.enumerated() {
            let btns = all(r, role: "AXButton").compactMap { s($0, kAXTitleAttribute as String) }.filter { !$0.isEmpty }
            let imgs = all(r, role: "AXImage")
            let desc = imgs.compactMap { s($0, kAXDescriptionAttribute as String) }.filter { !$0.isEmpty }
            let ta = all(r, role: "AXTextArea").compactMap { s($0, kAXValueAttribute as String) }.filter { !$0.isEmpty }
            let st = texts(r)
            if json {
                let b: String = btns.map(jstr).joined(separator: ",")
                let t2: String = ta.map(jstr).joined(separator: ",")
                let d: String = desc.map(jstr).joined(separator: ",")
                let tx: String = st.map { "{\"k\":" + jstr($0.0) + ",\"v\":" + jstr($0.1) + "}" }.joined(separator: ",")
                var o = "{\"i\":" + String(i)
                o += ",\"buttons\":[" + b + "]"
                o += ",\"textareas\":[" + t2 + "]"
                o += ",\"images\":" + String(imgs.count)
                o += ",\"imgdesc\":[" + d + "]"
                o += ",\"texts\":[" + tx + "]}"
                print(o)
            } else {
                print("\(i): imgs=\(imgs.count) buttons=\(btns.joined(separator: "|"))")
                for t in st where !t.1.isEmpty { print("     [\(t.0)] \(t.1.prefix(100))") }
                for t in ta { print("     <ta> \(t.prefix(100))") }
            }
        }
        break
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
case "find":
    guard let needle = args.dropFirst().first, !needle.isEmpty else { print("usage: find <substr>"); exit(2) }
    for e in all(ax) {
        let title = s(e, kAXTitleAttribute as String) ?? ""
        let val = s(e, kAXValueAttribute as String) ?? ""
        let id = s(e, kAXIdentifierAttribute as String) ?? ""
        if title.contains(needle) || val.contains(needle) {
            let role = s(e, kAXRoleAttribute as String) ?? "?"
            let p = point(e, kAXPositionAttribute as String).map { "(\(Int($0.x)),\(Int($0.y)))" } ?? "-"
            let sz = size(e).map { "(\(Int($0.width))x\(Int($0.height)))" } ?? "-"
            print("\(role) id=\(id) pos=\(p) size=\(sz) title=\(title) value=\(val)")
        }
    }
case "clickpt":
    let x = Double(args.dropFirst().first ?? "") ?? 0
    let y = Double(args.dropFirst(2).first ?? "") ?? 0
    app.activate(options: [.activateAllWindows])
    Thread.sleep(forTimeInterval: 0.5)
    let src = CGEventSource(stateID: .combinedSessionState)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left)?.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.9)
    click(CGPoint(x: x, y: y))
    Thread.sleep(forTimeInterval: 0.8)
    print("clicked(hover) at (\(x),\(y))")
case "press":
    guard let needle = args.dropFirst().first, !needle.isEmpty else { print("usage: press <title-substr>"); exit(2) }
    var hit: AXUIElement? = nil
    for e in all(ax) {
        let role = s(e, kAXRoleAttribute as String) ?? ""
        guard role == "AXButton" || role == "AXMenuItem" else { continue }
        let title = s(e, kAXTitleAttribute as String) ?? ""
        let val = s(e, kAXValueAttribute as String) ?? ""
        if title.contains(needle) || val.contains(needle) { hit = e; break }
    }
    guard let h = hit else { print("no button matching '\(needle)'"); exit(3) }
    let r = AXUIElementPerformAction(h, kAXPressAction as CFString)
    Thread.sleep(forTimeInterval: 1.0)
    print("pressed '\(needle)' rc=\(r.rawValue)")
case "rclick", "rclickpt":
    let rows = messageRows()
    if cmd == "rclick" {
        let fx = Double(args.dropFirst().first ?? "0.5") ?? 0.5
        let fy = Double(args.dropFirst(2).first ?? "0.5") ?? 0.5
        guard let win = all(ax, role: "AXWindow").first, let p0 = point(win, kAXPositionAttribute as String), let sz = size(win) else { print("no window"); exit(3) }
        app.activate(options: [.activateAllWindows]); Thread.sleep(forTimeInterval: 0.5)
        rightClick(CGPoint(x: p0.x + sz.width * fx, y: p0.y + sz.height * fy))
        print("right-clicked frac (\(fx),\(fy))")
    } else {
        let x = Double(args.dropFirst().first ?? "") ?? 0
        let y = Double(args.dropFirst(2).first ?? "") ?? 0
        app.activate(options: [.activateAllWindows]); Thread.sleep(forTimeInterval: 0.5)
        rightClick(CGPoint(x: x, y: y))
        print("right-clicked pt (\(x),\(y))")
    }
    _ = rows
case "row":
    guard let i = Int(args.dropFirst().first ?? "") else { print("usage: row <idx>"); exit(2) }
    let rows = messageRows()
    guard i < rows.count else { print("out of range (\(rows.count))"); exit(2) }
    func walk(_ e: AXUIElement, _ d: Int) {
        let role = s(e, kAXRoleAttribute as String) ?? "?"
        var bits = [role]
        for k in [kAXTitleAttribute, kAXValueAttribute, kAXIdentifierAttribute, kAXDescriptionAttribute] {
            if let v = s(e, k as String), !v.isEmpty { bits.append("\(k)=\(v.prefix(60))") }
        }
        let p = point(e, kAXPositionAttribute as String).map { "(\(Int($0.x)),\(Int($0.y)))" } ?? "-"
        let sz = size(e).map { "\(Int($0.width))x\(Int($0.height))" } ?? "-"
        bits.append("pos=\(p)")
        bits.append("size=\(sz)")
        print(String(repeating: "  ", count: d) + bits.joined(separator: " | "))
        for k in kids(e) { walk(k, d + 1) }
    }
    walk(rows[i], 0)
case "pressrow":
    guard let i = Int(args.dropFirst().first ?? "") else { print("usage: pressrow <idx> <title-substr>"); exit(2) }
    let needle = args.dropFirst(2).first ?? ""
    let rows = messageRows()
    guard i < rows.count else { print("out of range (\(rows.count))"); exit(2) }
    var hit: AXUIElement? = nil
    for e in all(rows[i], role: "AXButton") {
        let t = s(e, kAXTitleAttribute as String) ?? ""
        if t.contains(needle) || s(e, kAXValueAttribute as String)?.contains(needle) == true { hit = e; break }
    }
    guard let h = hit else { print("no button '\(needle)' in row \(i)"); exit(3) }
    let rc = AXUIElementPerformAction(h, kAXPressAction as CFString)
    print("pressrow \(i) '\(needle)' rc=\(rc.rawValue)")
case "move":
    let x = Double(args.dropFirst().first ?? "") ?? 0
    let y = Double(args.dropFirst(2).first ?? "") ?? 0
    app.activate(options: [.activateAllWindows]); Thread.sleep(forTimeInterval: 0.4)
    let src = CGEventSource(stateID: .combinedSessionState)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: CGPoint(x: x, y: y), mouseButton: .left)?.post(tap: .cghidEventTap)
    print("moved to (\(x),\(y))")
case "pressin":
    guard let i = Int(args.dropFirst().first ?? "") else { print("usage: pressin <rowIdx> <text>"); exit(2) }
    let needle = args.dropFirst(2).first ?? ""
    let rows = messageRows()
    guard i < rows.count else { print("out of range (\(rows.count))"); exit(2) }
    var hit: AXUIElement? = nil
    for e in all(rows[i]) {
        let t = s(e, kAXTitleAttribute as String) ?? ""
        let v = s(e, kAXValueAttribute as String) ?? ""
        if t.contains(needle) || v.contains(needle) { hit = e; break }
    }
    guard let h = hit else { print("no elem '\(needle)' in row \(i)"); exit(3) }
    let role = s(h, kAXRoleAttribute as String) ?? "?"
    let rc = AXUIElementPerformAction(h, kAXPressAction as CFString)
    print("pressin \(i) '\(needle)' role=\(role) rc=\(rc.rawValue)")
case "pressany":
    let needle = args.dropFirst().first ?? ""
    var out: [String] = []
    for e in all(ax) {
        let t = s(e, kAXTitleAttribute as String) ?? ""
        let v = s(e, kAXValueAttribute as String) ?? ""
        if (t.contains(needle) || v.contains(needle)) {
            let rc = AXUIElementPerformAction(e, kAXPressAction as CFString)
            out.append("\(s(e, kAXRoleAttribute as String) ?? "?") rc=\(rc.rawValue)")
        }
    }
    print(out.isEmpty ? "no match" : out.joined(separator: "; "))
case "chord":
    let codes = args.dropFirst().compactMap { UInt16($0) }
    guard !codes.isEmpty else { print("usage: chord <keycode> [<keycode>...]"); exit(2) }
    app.activate(options: [.activateIgnoringOtherApps, .activateAllWindows])
    Thread.sleep(forTimeInterval: 0.5)
    let src = CGEventSource(stateID: .hidSystemState)
    for c in codes { CGEvent(keyboardEventSource: src, virtualKey: c, keyDown: true)?.postToPid(app.processIdentifier); usleep(40_000) }
    for c in codes.reversed() { CGEvent(keyboardEventSource: src, virtualKey: c, keyDown: false)?.postToPid(app.processIdentifier); usleep(40_000) }
    Thread.sleep(forTimeInterval: 0.8)
    print("chord \(codes)")
case "btns":
    for e in all(ax, role: "AXButton") {
        let t = s(e, kAXTitleAttribute as String) ?? ""
        let id = s(e, kAXIdentifierAttribute as String) ?? ""
        let p = point(e, kAXPositionAttribute as String).map { "(\(Int($0.x)),\(Int($0.y)))" } ?? "-"
        let sz = size(e).map { "\(Int($0.width))x\(Int($0.height))" } ?? "-"
        print("\(p) \(sz) id=\(id) title=\(t)")
    }
case "probe":
    let needle = args.dropFirst().first ?? ""
    func acts(_ e: AXUIElement) -> [String] {
        var names: CFArray?
        guard AXUIElementCopyActionNames(e, &names) == .success, let arr = names as? [String] else { return [] }
        return arr
    }
    var n = 0
    for e in all(ax) {
        let t = s(e, kAXTitleAttribute as String) ?? ""
        let v = s(e, kAXValueAttribute as String) ?? ""
        guard t.contains(needle) || v.contains(needle) else { continue }
        n += 1
        if n > 6 { break }
        print("== match: role=\(s(e, kAXRoleAttribute as String) ?? "?") actions=\(acts(e)) pos=\(String(describing: point(e, kAXPositionAttribute as String)))")
        var cur: AXUIElement? = e
        for up in 1...4 {
            guard let c = cur, let par = attr(c, kAXParentAttribute as String) as! AXUIElement? else { break }
            print("   ^\(up) role=\(s(par, kAXRoleAttribute as String) ?? "?") id=\(s(par, kAXIdentifierAttribute as String) ?? "") actions=\(acts(par))")
            cur = par
        }
    }
    if n == 0 { print("no match '\(needle)'") }
case "doact":
    guard let i = Int(args.dropFirst().first ?? "") else { print("usage: doact <rowIdx> <text> <action>"); exit(2) }
    let needle = args.dropFirst(2).first ?? ""
    let action = args.dropFirst(3).first ?? kAXPressAction as String
    let rows = messageRows()
    guard i < rows.count else { print("out of range"); exit(2) }
    var hit: AXUIElement? = nil
    for e in all(rows[i]) { let v = s(e, kAXValueAttribute as String) ?? ""; let t = s(e, kAXTitleAttribute as String) ?? ""; if v.contains(needle) || t.contains(needle) { hit = e; break } }
    guard let h = hit else { print("no elem"); exit(3) }
    var tryEls: [AXUIElement] = [h]
    var cur: AXUIElement? = h
    for _ in 1...3 { if let c = cur, let par = attr(c, kAXParentAttribute as String) as! AXUIElement? { tryEls.append(par); cur = par } else { break } }
    for e in tryEls {
        let rc = AXUIElementPerformAction(e, action as CFString)
        print("role=\(s(e, kAXRoleAttribute as String) ?? "?") action=\(action) rc=\(rc.rawValue)")
        if rc == .success { break }
    }
case "clickpid":
    let x = Double(args.dropFirst().first ?? "") ?? 0
    let y = Double(args.dropFirst(2).first ?? "") ?? 0
    let pt = CGPoint(x: x, y: y)
    app.activate(options: [.activateAllWindows]); Thread.sleep(forTimeInterval: 0.5)
    let pid = app.processIdentifier
    let src = CGEventSource(stateID: .hidSystemState)
    if let m = CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: pt, mouseButton: .left) { m.postToPid(pid) }
    usleep(120_000)
    if let d = CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: pt, mouseButton: .left) { d.postToPid(pid) }
    usleep(120_000)
    if let u = CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: pt, mouseButton: .left) { u.postToPid(pid) }
    Thread.sleep(forTimeInterval: 0.8)
    print("clickpid at (\(x),\(y))")
case "at":
    let x = Float(args.dropFirst().first ?? "") ?? 0
    let y = Float(args.dropFirst(2).first ?? "") ?? 0
    var el: AXUIElement?
    let sysw = AXUIElementCreateSystemWide()
    let rc = AXUIElementCopyElementAtPosition(sysw, x, y, &el)
    print("rc=\(rc.rawValue)")
    if let e = el {
        print("role=\(s(e, kAXRoleAttribute as String) ?? "?") id=\(s(e, kAXIdentifierAttribute as String) ?? "") title=\(s(e, kAXTitleAttribute as String) ?? "") value=\(s(e, kAXValueAttribute as String) ?? "")")
        if let par = attr(e, kAXParentAttribute as String) as! AXUIElement? {
            print("parent role=\(s(par, kAXRoleAttribute as String) ?? "?") title=\(s(par, kAXTitleAttribute as String) ?? "")")
        }
    }
case "pressat":
    let x = Double(args.dropFirst().first ?? "") ?? 0
    let y = Double(args.dropFirst(2).first ?? "") ?? 0
    var best: AXUIElement? = nil; var bd = 1e9
    for e in all(ax, role: "AXButton") {
        guard let p = point(e, kAXPositionAttribute as String), let sz = size(e) else { continue }
        let cx = p.x + sz.width/2, cy = p.y + sz.height/2
        let d = sqrt((cx-x)*(cx-x) + (cy-y)*(cy-y))
        if d < bd { bd = d; best = e }
    }
    if let b = best {
        let rc = AXUIElementPerformAction(b, kAXPressAction as CFString)
        print("pressat nearest dist=\(Int(bd)) role=\(s(b, kAXRoleAttribute as String) ?? "?") title=\(s(b, kAXTitleAttribute as String) ?? "") rc=\(rc.rawValue)")
    } else { print("no button") }
case "menus":
    for e in all(ax) {
        let role = s(e, kAXRoleAttribute as String) ?? ""
        guard role == "AXMenuItem" || role == "AXMenu" || role == "AXStaticText" else { continue }
        let t = s(e, kAXTitleAttribute as String) ?? ""
        let v = s(e, kAXValueAttribute as String) ?? ""
        let lbl = t.isEmpty ? v : t
        guard !lbl.isEmpty else { continue }
        if let p = point(e, kAXPositionAttribute as String) {
            print("\(role) \(Int(p.x)),\(Int(p.y)) \(lbl)")
        }
    }
case "dblclickpt":
    let x = Double(args.dropFirst().first ?? "") ?? 0
    let y = Double(args.dropFirst(2).first ?? "") ?? 0
    let pt = CGPoint(x: x, y: y)
    app.activate(options: [.activateAllWindows]); Thread.sleep(forTimeInterval: 0.5)
    let src = CGEventSource(stateID: .combinedSessionState)
    CGEvent(mouseEventSource: src, mouseType: .mouseMoved, mouseCursorPosition: pt, mouseButton: .left)?.post(tap: .cghidEventTap)
    Thread.sleep(forTimeInterval: 0.5)
    for c in 1...2 {
        let d = CGEvent(mouseEventSource: src, mouseType: .leftMouseDown, mouseCursorPosition: pt, mouseButton: .left); d?.setIntegerValueField(.mouseEventClickState, value: Int64(c)); d?.post(tap: .cghidEventTap)
        usleep(40_000)
        let u = CGEvent(mouseEventSource: src, mouseType: .leftMouseUp, mouseCursorPosition: pt, mouseButton: .left); u?.setIntegerValueField(.mouseEventClickState, value: Int64(c)); u?.post(tap: .cghidEventTap)
        usleep(60_000)
    }
    Thread.sleep(forTimeInterval: 1.0)
    print("dblclick at (\(x),\(y))")
case "pressid":
    let needle = args.dropFirst().first ?? ""
    var hit: AXUIElement? = nil
    for e in all(ax) {
        if (s(e, kAXIdentifierAttribute as String) ?? "") == needle { hit = e; break }
    }
    guard let h = hit else { print("no element id=\(needle)"); exit(3) }
    let rc = AXUIElementPerformAction(h, kAXPressAction as CFString)
    let pos = point(h, kAXPositionAttribute as String).map { "(\(Int($0.x)),\(Int($0.y)))" } ?? "-"
    print("pressid \(needle) role=\(s(h, kAXRoleAttribute as String) ?? "?") pos=\(pos) rc=\(rc.rawValue)")
case "ids":
    let pre = args.dropFirst().first ?? "_NS:"
    for e in all(ax) {
        let id = s(e, kAXIdentifierAttribute as String) ?? ""
        guard id.hasPrefix(pre) else { continue }
        let role = s(e, kAXRoleAttribute as String) ?? "?"
        let pos = point(e, kAXPositionAttribute as String).map { "(\(Int($0.x)),\(Int($0.y)))" } ?? "-"
        let sz = size(e).map { "\(Int($0.width))x\(Int($0.height))" } ?? "-"
        print("\(id) \(role) \(pos) \(sz) title=\(s(e, kAXTitleAttribute as String) ?? "")")
    }
case "imgmap":
    let rows = messageRows()
    for (i, r) in rows.enumerated() {
        let imgs = all(r, role: "AXImage")
        if imgs.isEmpty { continue }
        let parts = imgs.map { e -> String in
            let p = point(e, kAXPositionAttribute as String).map { "(\(Int($0.x)),\(Int($0.y)))" } ?? "-"
            let sz = size(e).map { "\(Int($0.width))x\(Int($0.height))" } ?? "-"
            return "\(p)\(sz)"
        }
        print("\(i): n=\(imgs.count) " + parts.joined(separator: " "))
    }
case "imgattrs":
    for (i, r) in messageRows().enumerated() {
        for e in all(r, role: "AXImage") {
            if let szv = size(e), szv.width < 100, szv.height < 100 { continue }
            var names: CFArray? = nil
            AXUIElementCopyAttributeNames(e, &names)
            var kv: [String] = []
            for n in (names as? [String] ?? []) {
                if let v = attr(e, n) { kv.append("\(n)=\(String(describing: v).prefix(180))") }
            }
            let szs = size(e).map { "\(Int($0.width))x\(Int($0.height))" } ?? "-"
            print("row \(i) \(szs) :: " + kv.joined(separator: " | "))
        }
    }
default:
    print("commands: sessions | chat | click <idx> | rows | collect | dump [depth] | find <s> | press <s> | scrollfrac <f>")
}
