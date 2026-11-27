// Tray.cs: the notify icon (app.ico at the small icon size), its Open / Exit menu, double-click opens, balloon
// notices. Owner: app shell.
#pragma once
#include <windows.h>

#include <functional>
#include <string>

constexpr UINT WM_APP_TRAY = WM_APP + 41;

class Tray {
public:
	Tray(HWND owner, std::function<void()> open, std::function<void()> exit);
	~Tray();
	Tray(const Tray&) = delete;
	Tray& operator=(const Tray&) = delete;
	/// A short notice from the tray icon (Windows shows it as a notification), error icon.
	void Notice(const std::string& text);
	/// A plain information notice (updates).
	void Info(const std::string& text);
	/// The window procedure forwards WM_APP_TRAY and the "TaskbarCreated" message here. True when handled.
	bool Handle(UINT msg, WPARAM wp, LPARAM lp);

private:
	void Add();
	void Balloon(const std::string& text, DWORD icon);
	void Menu();
	HWND hwnd_;
	HICON icon_ = nullptr;
	UINT taskbarCreated_ = 0;
	std::function<void()> open_, exit_;
	bool added_ = false;
};
