// ErrorPopup.cs: errors are shown in a Windows error window as soon as they happen (there is no on-screen journal).
// While a game runs under Auto the window waits until the game is closed, so it never pulls a player out of a
// match; the tray shows a short notice at once. Errors that come in while the window is open are added to the next
// one. Owner: app shell.
#pragma once
#include <windows.h>

#include <functional>
#include <string>
#include <vector>

class ErrorPopup {
public:
	struct Entry {
		std::string time;   // HH:mm:ss, local
		std::string msg;
	};
	ErrorPopup(std::function<bool()> gameRunning, std::function<HWND()> owner, std::function<void(const std::string&)> trayNotice);
	/// AppLog.ErrorAdded.
	void OnError(const Entry& e);
	/// Called from the UI loop (outside a frame): the 3 s wait timer and the re-try after a closed window.
	void Poll(double now);

private:
	bool Running();
	void TryShow(double now);
	std::function<bool()> gameRunning_;
	std::function<HWND()> owner_;
	std::function<void(const std::string&)> trayNotice_;
	std::vector<Entry> pending_;
	bool waiting_ = false;
	double nextWait_ = 0;
	bool showing_ = false;
	bool retry_ = false;
};
