// Shell.cs: the window's content (1.14.0, HUD look): our own title bar (logo, status pill with the session time,
// the Advanced mode switch, settings, the caption buttons), then either the simple home screen with its footer or
// the advanced layout (numbered page list at the left, the page, status and "This system" at the right).
// Owner: app shell.
#pragma once
#include <windows.h>

#include <cstdint>
#include <functional>
#include <memory>
#include <string>
#include <vector>

#include "core.h"
#include "page.h"
#include "ui.h"

class Shell {
public:
	explicit Shell(HWND hwnd);
	~Shell();

	/// Draws the whole window (call between ImGui::NewFrame and Render).
	void Draw();
	/// Shell.Refresh: reads the core's state, updates the header / status column, ticks the page on screen.
	void Refresh();
	/// The core's state version changed (also while the window is hidden): facts, language, jobs.
	void OnCoreChanged();
	/// Polls running Busy jobs (from the UI loop).
	void PollJobs();

	// ---- the API the pages use (Shell.cs public members)
	/// Advanced mode shows every page; the simple mode only the home screen and the settings.
	bool Simple() const { return simple_; }
	float DpiScaleX() const;
	bool AutoOn() const;
	void SetSimple(bool simple, bool resize = true);
	void Show(int index);
	void ShowPage(PageId id);
	Page* Get(PageId id);
	/// Busy / BusyTask: shows the overlay with `text` until the core's job `job` (dos_job_poll) is done, then calls
	/// done with its JSON result and refreshes.
	void Busy(const std::string& text, uint64_t job, std::function<void(const json::Value&)> done);
	/// Did: AppLog.Add(message, sev) and Refresh.
	void Did(const std::string& message, core::Sev sev = core::Sev::Info);
	/// The last state the core gave (dos_shell_state) and its facts (dos_facts).
	const json::Value& State() const { return state_; }
	const json::Value& Facts() const { return facts_; }

	// ---- the window
	void SaveGeometry();
	/// Places the window (Shell's RestoreGeometry + the start size); before the first show.
	void RestoreGeometry();
	/// Visible / minimized changed (IsVisibleChanged, StateChanged).
	void OnWindowState();
	/// The DWM border (Chrome.Dwm on SourceInitialized).
	void OnCreated();
	/// MinWidth / MinHeight in DIP.
	ImVec2 MinSizeDip() const { return simple_ ? ImVec2(860, 620) : ImVec2(1180, 700); }
	/// The caption height in client px (for WM_NCHITTEST).
	float CaptionPx() const;
	bool Maximized() const;

private:
	struct Step {
		std::string text;
		bool done;
		bool warn;
	};
	struct Job {
		uint64_t id;
		std::function<void(const json::Value&)> done;
	};

	void DrawHeader(ImRect r);
	void DrawAdvanced(ImRect r);
	void DrawSimple(ImRect r);
	void DrawStatus(ImRect r, float& usedH);
	void DrawNav(ImRect r);
	void DrawOverlay(ImRect body);
	void DrawPage(ImRect r);
	void ShowSimpleSettings(bool on);
	void UndoEverything();
	void SimpleTexts();
	void Texts();
	void OnLanguage();
	void RefreshCore();
	void Pill();
	bool OnScreen() const;
	int FindPage(PageId id) const;
	void HidePage(int index);
	ui::Thick Padding() const;
	void ResizeDip(float w, float h);

	HWND hwnd_;
	std::vector<std::unique_ptr<Page>> pages_;
	std::unique_ptr<HomeViewBase> home_;
	std::unique_ptr<SystemPanelBase> sysAdv_;
	int current_ = 0;
	bool simple_ = false;
	bool simpleSettings_ = false;
	bool dwmBorder_ = false;
	bool closedShown_ = false;
	std::string pageError_;   // the message shown when a page failed to build

	json::Value state_;
	json::Value facts_;
	std::string factsKey_;

	// header pill
	int dotState_ = -1;
	double activeSince_ = 0;
	const char* dotColor_ = "Faint";
	const char* haloColor_ = "None";
	const char* wordColor_ = "Dim";
	const char* pillLine_ = "Line";
	std::string pillWord_, pillTime_, pillSep_, pillTip_;

	// status column
	std::string stateBig_, stateSub_;
	const char* stateBigBrush_ = "Strong";
	bool toggleOn_ = false;          // auto.On
	bool toggleEnabled_ = true;      // auto.ButtonEnabled (false right after a click)
	bool toggleSemi_ = false;        // Ui.Primary sets SemiBold and never clears it
	std::vector<Step> steps_;
	std::string nThreads_ = "0", nLowered_ = "0", nChanged_ = "0";

	// footers
	std::string foot_, simpleFoot_, simpleFootTip_;

	// overlay
	std::vector<Job> jobs_;
	std::string overlayText_;
	bool overlay_ = false;

	std::string lang_;
	int pageBuiltFor_ = -1;
};

/// The one shell of the app (set by main).
Shell* TheShell();
void SetTheShell(Shell* s);
