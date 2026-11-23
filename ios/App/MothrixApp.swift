// SPDX-License-Identifier: MPL-2.0
//
// The app: starts mothrix, asks for push notifications, and shows the sign-in
// or the room list.

import SwiftUI
import UserNotifications

@main
struct MothrixApp: App {
    @UIApplicationDelegateAdaptor(AppDelegate.self) var delegate
    @StateObject private var store = Store()
    @Environment(\.scenePhase) private var phase

    var body: some Scene {
        WindowGroup {
            Root()
                .environmentObject(store)
                .onAppear {
                    delegate.store = store
                    store.start()
                }
                .onChange(of: phase) { _, phase in
                    store.focus(phase == .active)
                }
        }
    }
}

struct Root: View {
    @EnvironmentObject var store: Store

    var body: some View {
        Group {
            if !store.started {
                // While Rust starts: the logo, as the terminal's start screen.
                VStack(spacing: 12) {
                    Moth(size: 280)
                    Text("mothrix").font(.mono(.title)).foregroundStyle(store.theme.accentColor)
                    ProgressView().tint(store.theme.accentColor)
                }
            } else if store.accounts.isEmpty {
                SignInView()
            } else {
                RoomList()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(store.theme.bg)
        .foregroundStyle(store.theme.text)
        .preferredColorScheme(.dark)
        .sheet(isPresented: Binding(get: { store.verify != nil }, set: { _ in })) {
            VerifyView().environmentObject(store)
        }
        .notices()
    }
}

extension View {
    /// Rust's notices at the bottom: on the main view and on every sheet
    /// (a sheet would cover the main view's).
    func notices() -> some View { modifier(Notices()) }
}

struct Notices: ViewModifier {
    @EnvironmentObject var store: Store

    func body(content: Content) -> some View {
        content.overlay(alignment: .bottom) {
            if let notice = store.notice {
                Text(notice)
                    .font(.mono(.footnote))
                    .padding(8)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(store.theme.line)
                    .onTapGesture { store.notice = nil }
                    .task(id: notice) {
                        try? await Task.sleep(for: .seconds(6))
                        if store.notice == notice { store.notice = nil }
                    }
            }
        }
    }
}

/// Push: permission, the token, and taps on notifications.
final class AppDelegate: NSObject, UIApplicationDelegate, UNUserNotificationCenterDelegate {
    weak var store: Store?

    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions options: [UIApplication.LaunchOptionsKey: Any]? = nil
    ) -> Bool {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        // What can be done right on a message's notification.
        let reply = UNTextInputNotificationAction(
            identifier: "reply", title: "Reply", options: [],
            textInputButtonTitle: "Send", textInputPlaceholder: "message")
        let read = UNNotificationAction(identifier: "read", title: "Mark as read", options: [])
        center.setNotificationCategories([
            UNNotificationCategory(identifier: "message", actions: [reply, read],
                                   intentIdentifiers: [], options: []),
        ])
        center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
            guard granted else { return }
            DispatchQueue.main.async { application.registerForRemoteNotifications() }
        }
        return true
    }

    func application(
        _ application: UIApplication,
        didRegisterForRemoteNotificationsWithDeviceToken token: Data
    ) {
        Task { @MainActor in self.store?.setPushToken(token) }
    }

    func application(
        _ application: UIApplication,
        didFailToRegisterForRemoteNotificationsWithError error: Error
    ) {
        print("mothrix: no push token: \(error)")
    }

    /// In the front, the room list shows what is new: no banner.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        [.badge]
    }

    /// A tap opens the room the extension wrote into the notification;
    /// "Reply" and "Mark as read" do their work without opening the app.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        guard let room = response.notification.request.content.userInfo["mothrix_room"] as? String
        else { return }
        switch response.actionIdentifier {
        case "reply":
            let text = (response as? UNTextInputNotificationResponse)?.userText ?? ""
            await Self.answer(room: room, text: text)
        case "read":
            await Self.answer(room: room, text: nil)
        case UNNotificationDefaultActionIdentifier:
            await MainActor.run { self.store?.openFromNotification(room) }
        default:
            break
        }
    }

    /// Send (if there is text) and read the room up to its newest message,
    /// through Rust's own short-lived client on the shared store: works
    /// whether the app runs or iOS just woke it for this. iOS is asked for
    /// the time it takes.
    private static func answer(room: String, text: String?) async {
        let folder = sharedFolder().path
        let task = await MainActor.run { UIApplication.shared.beginBackgroundTask() }
        let failed: String? = await withCheckedContinuation { done in
            DispatchQueue.global(qos: .userInitiated).async {
                do {
                    try respond(folder: folder, room: room, text: text)
                    done.resume(returning: nil)
                } catch {
                    done.resume(returning: "\(error)")
                }
            }
        }
        Store.clearNotifications(of: [room])
        if let failed, text != nil {
            // Say so: the answer did not go out.
            let note = UNMutableNotificationContent()
            note.title = "Not sent"
            note.body = "Your answer could not be sent: \(failed)"
            note.threadIdentifier = room
            note.userInfo["mothrix_room"] = room
            try? await UNUserNotificationCenter.current().add(
                UNNotificationRequest(identifier: UUID().uuidString, content: note, trigger: nil))
        }
        await MainActor.run { UIApplication.shared.endBackgroundTask(task) }
    }
}
