// Services of the application window that widgets and pages use (port of the bits of App/Window they relied on).
// Owner: app shell.
#pragma once
#include <windows.h>

#include <d3d11.h>

#include <cstdint>
#include <string>

namespace app {

HWND Hwnd();
ID3D11Device* Device();

/// Draws another frame as soon as possible (posts a message to the UI thread; safe from any thread).
void Redraw();
/// Asks for a frame after `ms` milliseconds (a page's timer tick, a tooltip delay, the confirm button's 4 s).
/// The earliest request wins; rendering stays on demand otherwise.
void WakeIn(int ms);
/// Seconds since the app started (monotonic), for timers that run inside frames.
double Now();
/// True when the window is on screen (visible, not minimized): only then anything is drawn.
bool OnScreen();

/// Shows the window (tray Open, double-click, the Show event): Show, restore if minimized, Activate.
void ShowWindow();
/// The tray's Exit / the session end: App.ExitApp.
void ExitApp();

/// Tray notices (Tray.Notice = error balloon, Tray.Info = information balloon).
void TrayNotice(const std::string& text);
void TrayInfo(const std::string& text);

} // namespace app
