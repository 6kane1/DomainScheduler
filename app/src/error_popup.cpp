#include "error_popup.h"

#include "app.h"
#include "core.h"
#include "ui.h"

ErrorPopup::ErrorPopup(std::function<bool()> gameRunning, std::function<HWND()> owner, std::function<void(const std::string&)> trayNotice)
	: gameRunning_(std::move(gameRunning)), owner_(std::move(owner)), trayNotice_(std::move(trayNotice))
{
}

void ErrorPopup::OnError(const Entry& e)
{
	bool first = pending_.empty();
	pending_.push_back(e);
	if (first && Running())
	{
		if (trayNotice_) trayNotice_(L::T("Domain OS Toolbox: an error happened. The details open when the game is closed."));
	}
	TryShow(app::Now());
}

bool ErrorPopup::Running() { return gameRunning_ ? gameRunning_() : false; }

void ErrorPopup::Poll(double now)
{
	if (retry_)
	{
		retry_ = false;
		TryShow(now);
		return;
	}
	if (waiting_ && now >= nextWait_)
	{
		nextWait_ = now + 3.0;   // the DispatcherTimer keeps ticking every 3 s while started
		TryShow(now);
	}
	if (waiting_) app::WakeIn((int)((nextWait_ - now) * 1000.0) + 5);
}

void ErrorPopup::TryShow(double now)
{
	if (showing_ || pending_.empty())
	{
		waiting_ = false;
		return;
	}
	if (Running())
	{
		if (!waiting_)
		{
			waiting_ = true;
			nextWait_ = now + 3.0;
			app::WakeIn(3005);
		}
		return;
	}
	waiting_ = false;
	std::vector<Entry> items = pending_;
	pending_.clear();
	std::string sb;
	size_t from = items.size() > 8 ? items.size() - 8 : 0;
	for (size_t i = from; i < items.size(); i++) sb += items[i].time + "  " + items[i].msg + "\r\n";
	if (items.size() > 8) sb += L::F("…and {0} earlier", {std::to_string(items.size() - 8)}) + "\r\n";
	sb += "\r\n";
	sb += L::F("Details are in the log file: {0}", {core::Take(dos_log_shown_path())});
	showing_ = true;
	HWND owner = nullptr;
	if (owner_)
	{
		HWND w = owner_();
		if (w != nullptr && IsWindowVisible(w)) owner = w;
	}
	std::wstring title = ui::Wide(L::T("Domain OS Toolbox — error"));
	MessageBoxW(owner, ui::Wide(sb).c_str(), title.c_str(), MB_OK | MB_ICONERROR);
	showing_ = false;
	if (!pending_.empty())
	{
		retry_ = true;   // BeginInvoke(TryShow, Background)
		app::Redraw();
	}
}
