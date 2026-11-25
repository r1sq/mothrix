// SPDX-License-Identifier: MPL-2.0
//
// The JSON Rust sends, decoded. The fixtures are written by mothrix-ffi's
// test `what_the_swift_side_decodes` (MOTHRIX_WRITE_FIXTURES=1 cargo test
// -p mothrix-ffi): when a Rust type changes, write them again and see
// whether this still decodes.

import XCTest
@testable import Mothrix

final class DecodingTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let here = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
        return try Data(contentsOf: here.appendingPathComponent("Fixtures/\(name).json"))
    }

    func testTheSnapshotDecodes() throws {
        let event = try JSONDecoder.mothrix.decode(MothrixEvent.self, from: fixture("snapshot"))
        guard case .snapshot(let snapshot) = event else { return XCTFail("\(event)") }
        XCTAssertEqual(snapshot.accounts.count, 2)
        XCTAssertEqual(snapshot.openRoom, "!a")
        XCTAssertFalse(snapshot.lines.isEmpty)
        XCTAssertEqual(snapshot.settings.theme.name, "mothrix")
        let first = snapshot.items[0]
        XCTAssertEqual(first.senderName, "Amy")
        XCTAssertEqual(first.reactions.first?.count, 2)
        guard case .text(let runs) = first.blocks[0] else { return XCTFail() }
        XCTAssertEqual(runs[0].styles, ["bold"])
        guard case .code(let language, _) = first.blocks[1] else { return XCTFail() }
        XCTAssertEqual(language, "rust")
        XCTAssertEqual(snapshot.items[1].reply ?? [], ["Amy", "Hello there"])
        XCTAssertEqual(snapshot.items[1].thread?.replies, 3)
        guard case .poll(let poll) = snapshot.items[3].kind else { return XCTFail() }
        XCTAssertEqual(poll.answers.map(\.text), ["Pizza", "Soup"])
        XCTAssertEqual(snapshot.items[4].kind, .dateDivider)
    }

    /// The fixture holds one event of every kind Rust sends (its test does
    /// not compile when a kind is added without a sample).
    func testEveryEventDecodes() throws {
        let events = try JSONDecoder.mothrix.decode([MothrixEvent].self, from: fixture("events"))
        XCTAssertGreaterThanOrEqual(events.count, 21)
        for event in events {
            if case .other(let type) = event { XCTFail("not understood: \(type)") }
            if case .timeline(_, _, _, let dropped) = event { XCTAssertEqual(dropped, 0) }
        }
        XCTAssertTrue(events.contains { if case .problems(let list) = $0 { return list.first?.profile == "old" }; return false })
        XCTAssertTrue(events.contains { if case .verify(.some(.emojis(let pairs))) = $0 { return pairs.count == 2 }; return false })
    }

    /// A message this app cannot read (a number past what it holds, a
    /// kind it does not know) is left out; the conversation stays.
    func testABadMessageDoesNotDropTheConversation() throws {
        let all = try JSONSerialization.jsonObject(with: fixture("events")) as? [[String: Any]] ?? []
        guard var timeline = all.last(where: { $0["type"] as? String == "timeline" }),
              var items = timeline["items"] as? [[String: Any]], var bad = items.first
        else { return XCTFail("no timeline with messages in the fixture") }
        let good = items.count
        bad["ts_ms"] = "never"
        items.insert(bad, at: 1)
        items.append(["kind": ["Riddle": 1]])
        timeline["items"] = items
        let event = try JSONDecoder.mothrix.decode(
            MothrixEvent.self, from: JSONSerialization.data(withJSONObject: timeline))
        guard case .timeline(_, _, let kept, let dropped) = event else { return XCTFail("\(event)") }
        XCTAssertEqual(kept.count, good)
        XCTAssertEqual(dropped, 2)
        XCTAssertEqual(kept.first?.senderName, "Amy")

        // The snapshot reads its messages the same way.
        var snapshot = try JSONSerialization.jsonObject(with: fixture("snapshot")) as? [String: Any] ?? [:]
        var inner = snapshot["snapshot"] as? [String: Any] ?? [:]
        inner["items"] = items
        snapshot["snapshot"] = inner
        let whole = try JSONDecoder.mothrix.decode(
            MothrixEvent.self, from: JSONSerialization.data(withJSONObject: snapshot))
        guard case .snapshot(let read) = whole else { return XCTFail("\(whole)") }
        XCTAssertEqual(read.items.count, good)
        XCTAssertEqual(read.dropped, 2)
    }
}
