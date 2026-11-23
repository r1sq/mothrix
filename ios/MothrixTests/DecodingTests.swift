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

    func testEveryEventDecodes() throws {
        let events = try JSONDecoder.mothrix.decode([MothrixEvent].self, from: fixture("events"))
        XCTAssertFalse(events.isEmpty)
        for event in events {
            if case .other(let type) = event { XCTFail("not understood: \(type)") }
        }
    }
}
