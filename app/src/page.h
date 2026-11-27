// Page.cs and the page registry. Owner: app shell. The page agents implement these interfaces in their own files
// and register them with DOS_REGISTER_PAGE / DOS_REGISTER_HOME / DOS_REGISTER_SYSTEM_PANEL; whatever is not
// registered is shown as a placeholder with its title.
//
//   class QuickPage : public Page {
//     std::string Title() override { return L::T("Overview"); }
//     void Build() override { ... }               // Page.Build: the page is shown again, rebuild its state
//     void Draw(ImRect area) override { ... }     // every frame: the content of the page host
//     void Tick() override { ... }                // Shell.Refresh: the core's state changed
//   };
//   DOS_REGISTER_PAGE(PageId::Quick, QuickPage)
#pragma once
#include <memory>
#include <string>

#include "imgui.h"
#include "imgui_internal.h"
#include "json.h"

class Shell;

class Page {
public:
	virtual ~Page() = default;
	/// The translated title (L::T of the C# English title).
	virtual std::string Title() = 0;
	/// "Scheduler" (default), "System", "Hardware" or "Settings": the section caption of the page list.
	virtual const char* Section() { return "Scheduler"; }
	/// True for pages that are closed while Auto runs (left out of the list, the shell goes to page 0).
	virtual bool ClosedInAuto() { return false; }
	/// Page.Build(): called each time the page is shown (WPF built a new element every time).
	virtual void Build() {}
	/// Draws the page into `area` (the page host, padding already applied). Called every frame it is on screen.
	virtual void Draw(ImRect area) = 0;
	/// Page.Tick(): from Shell.Refresh while the page is on screen.
	virtual void Tick() {}
	/// Page.Shown(): right after Build.
	virtual void Shown() {}
	/// The page left the screen (another page was shown, the simple layout, the window hid or was minimized).
	/// Native-only: the Thread map stops its kernel trace here. Draw() is called again when it is back.
	virtual void Hidden() {}
	/// Native-only: the page takes the right column too (no status / "This system" column), like the Thread map.
	virtual bool Wide() { return false; }

	Shell* S = nullptr;   // set by the shell before Build
};

/// The pages in the order Shell's constructor adds them (the numbers of the page list follow this order).
enum class PageId {
	Quick,
	Masks,
	GameThreads,
	ThreadMap,   // native-only (no C# page): the Thread map, app/src/pages/thread_map.cpp
	BlackBox,
	Processes,
	Changes,
	Check,
	Gpu,
	Drivers,
	Power,
	Devices,
	Settings,
	Update,
	Count
};

/// SystemPanel.cs ("This system" in the right column and on the home screen).
class SystemPanelBase {
public:
	virtual ~SystemPanelBase() = default;
	virtual void Draw(ImRect area) = 0;
	virtual void Texts() {}
	/// SetFacts(Hardware.Facts): the core's facts JSON (dos_facts, "hardware" holds Hardware.Facts).
	virtual void SetFacts(const json::Value& facts) {}
};

/// HomeView.cs (the simple layout's home screen).
class HomeViewBase {
public:
	virtual ~HomeViewBase() = default;
	virtual void Draw(ImRect area) = 0;
	virtual void Texts() {}
	virtual void SetVisible(bool on) {}
	virtual void Tick() {}
	/// HomeView.SysPanel.
	virtual SystemPanelBase* SysPanel() { return nullptr; }
	Shell* S = nullptr;
};

using PageFactory = std::unique_ptr<Page> (*)();
using HomeFactory = std::unique_ptr<HomeViewBase> (*)();
using SystemPanelFactory = std::unique_ptr<SystemPanelBase> (*)();

namespace pages {
void Register(PageId id, PageFactory f);
void RegisterHome(HomeFactory f);
void RegisterSystemPanel(SystemPanelFactory f);
/// The page for an id: the registered one or a placeholder showing its title.
std::unique_ptr<Page> Make(PageId id);
std::unique_ptr<HomeViewBase> MakeHome();
std::unique_ptr<SystemPanelBase> MakeSystemPanel();
}

struct PageRegistrar {
	PageRegistrar(PageId id, PageFactory f) { pages::Register(id, f); }
	PageRegistrar(HomeFactory f) { pages::RegisterHome(f); }
	PageRegistrar(SystemPanelFactory f) { pages::RegisterSystemPanel(f); }
};

#define DOS_PAGE_CAT2(a, b) a##b
#define DOS_PAGE_CAT(a, b) DOS_PAGE_CAT2(a, b)
#define DOS_REGISTER_PAGE(ID, CLASS) \
	static PageRegistrar DOS_PAGE_CAT(g_page_reg_, __LINE__)(ID, []() -> std::unique_ptr<Page> { return std::make_unique<CLASS>(); });
#define DOS_REGISTER_HOME(CLASS) \
	static PageRegistrar DOS_PAGE_CAT(g_home_reg_, __LINE__)(static_cast<HomeFactory>([]() -> std::unique_ptr<HomeViewBase> { return std::make_unique<CLASS>(); }));
#define DOS_REGISTER_SYSTEM_PANEL(CLASS) \
	static PageRegistrar DOS_PAGE_CAT(g_sys_reg_, __LINE__)(static_cast<SystemPanelFactory>([]() -> std::unique_ptr<SystemPanelBase> { return std::make_unique<CLASS>(); }));
