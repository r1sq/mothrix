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
                    // A notification tapped before there was a store (the
                    // app was not running): its room opens now.
                    if let room = delegate.takeWaiting() {
                        store.openFromNotification(room)
                    }
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
                    if let problem = store.startProblem {
                        Text(problem)
                            .font(.mono(.footnote))
                            .foregroundStyle(store.theme.errorColor)
                            .multilineTextAlignment(.center)
                            .padding(.horizontal)
                    } else {
                        ProgressView().tint(store.theme.accentColor)
                    }
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
    /// The room of a notification tapped before `store` was set.
    private var waiting: String?

    /// That room, once.
    func takeWaiting() -> String? {
        let room = waiting
        waiting = nil
        return room
    }

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
            DispatchQueue.main.async {
                // Not when the "notifications" switch is known to be off
                // (the store follows the switch from then on).
                if self.store?.prefs?.notifications != false {
                    application.registerForRemoteNotifications()
                }
            }
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

    /// In the front, the room list shows what is new: no banner, and the
    /// icon's number stays the app's own (the push brings its server's
    /// count, also for a message the open room has read already; the
    /// notification extension has kept that count, so the app says its
    /// own again). Only our own word that an answer did not go out is
    /// shown here too.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter, willPresent notification: UNNotification
    ) async -> UNNotificationPresentationOptions {
        if notification.request.content.userInfo["mothrix_problem"] != nil {
            return [.banner, .list]
        }
        await MainActor.run {
            if let store = self.store { store.showBadge(again: true) }
        }
        return []
    }

    /// A tap opens the room the extension wrote into the notification;
    /// "Reply" and "Mark as read" do their work without opening the app.
    func userNotificationCenter(
        _ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse
    ) async {
        let info = response.notification.request.content.userInfo
        guard let room = info["mothrix_room"] as? String else { return }
        // The thread the message is in: an answer goes there too.
        let thread = info["mothrix_thread"] as? String
        switch response.actionIdentifier {
        case "reply":
            let text = (response as? UNTextInputNotificationResponse)?.userText ?? ""
            await Self.answer(room: room, text: text, thread: thread)
        case "read":
            await Self.answer(room: room, text: nil, thread: nil)
        case UNNotificationDefaultActionIdentifier:
            await MainActor.run {
                if let store = self.store {
                    store.openFromNotification(room)
                } else {
                    self.waiting = room
                }
            }
        default:
            break
        }
    }

    /// Send (if there is text) and read the room up to its newest message.
    /// Rust's `respond` has the running app's accounts do it; when iOS
    /// just woke the app for this and none run yet, it uses a short-lived
    /// client of its own on the shared store. iOS is asked for the time
    /// it takes.
    private static func answer(room: String, text: String?, thread: String?) async {
        let folder = sharedFolder().path
        let task = await MainActor.run { UIApplication.shared.beginBackgroundTask() }
        let failed: String? = await withCheckedContinuation { done in
            DispatchQueue.global(qos: .userInitiated).async {
                // (iOS may have started the app just for this.)
                StoreKey.handOver()
                do {
                    try respond(folder: folder, room: room, text: text, thread: thread)
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
            // Shown also while the app is in front (`willPresent`).
            note.userInfo["mothrix_problem"] = true
            try? await UNUserNotificationCenter.current().add(
                UNNotificationRequest(identifier: UUID().uuidString, content: note, trigger: nil))
        }
        await MainActor.run { UIApplication.shared.endBackgroundTask(task) }
    }
}
