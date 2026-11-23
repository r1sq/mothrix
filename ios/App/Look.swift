// SPDX-License-Identifier: MPL-2.0
//
// mothrix's look on the phone: the theme's colours, a monospace font, the
// moth, and words in brackets instead of icons, as in the terminal.

import SwiftUI

extension Color {
    /// `#rrggbb`; grey if it is not one.
    init(hex: String) {
        let digits = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        let value = UInt32(digits, radix: 16) ?? 0x808080
        self.init(
            red: Double((value >> 16) & 0xff) / 255,
            green: Double((value >> 8) & 0xff) / 255,
            blue: Double(value & 0xff) / 255)
    }
}

extension Palette {
    var bg: Color { Color(hex: background) }
    var text: Color { Color(hex: fg) }
    var faint: Color { Color(hex: dim) }
    var accentColor: Color { Color(hex: accent) }
    var line: Color { Color(hex: border) }
    var unreadColor: Color { Color(hex: unread) }
    var mentionColor: Color { Color(hex: mention) }
    var errorColor: Color { Color(hex: error) }
    var successColor: Color { Color(hex: success) }
}

extension Font {
    static func mono(_ style: Font.TextStyle = .body) -> Font {
        .system(style, design: .monospaced)
    }
}

/// A word that does something, like `[reply]` in the terminal.
struct WordButton: View {
    @EnvironmentObject var store: Store
    let word: String
    let action: () -> Void

    init(_ word: String, action: @escaping () -> Void) {
        self.word = word
        self.action = action
    }

    var body: some View {
        Button(action: action) {
            Text(word)
                .font(.mono(.footnote))
                .foregroundStyle(store.theme.accentColor)
                .lineLimit(1)
                .fixedSize()
        }
        .buttonStyle(.plain)
    }
}

/// The moth of the start screen, in the theme's colours.
/// The mothrix logo (the moth before a ringed planet, on a transparent
/// background): in the room list's header and behind its rooms, while
/// starting and on the sign-in screen.
struct Moth: View {
    var size: CGFloat = 220

    var body: some View {
        Image("Logo")
            .resizable()
            .interpolation(.high)
            .scaledToFit()
            .frame(width: size, height: size)
            .accessibilityLabel("mothrix")
    }
}
