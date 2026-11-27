// The page registry and the placeholders shown for pages no page agent registered yet. Owner: app shell.
#include <array>

#include "core.h"
#include "page.h"
#include "ui.h"

namespace {

struct Registry {
	std::array<PageFactory, (size_t)PageId::Count> pages{};
	HomeFactory home = nullptr;
	SystemPanelFactory sys = nullptr;
};

// Static registrars run before main in any order: a function-local static avoids the order problem.
Registry& Reg()
{
	static Registry r;
	return r;
}

struct Meta {
	const char* title;     // the English title (L.T key) of the C# page
	const char* section;   // its Section
};

// Shell's constructor order and the Title / Section of each C# page.
const Meta kMeta[(size_t)PageId::Count] = {
	{"Overview", "Scheduler"},              // QuickPage
	{"Masks", "Scheduler"},                 // MasksPage
	{"Game threads", "Scheduler"},          // GameThreadsPage
	{"Thread map", "Scheduler"},            // native-only
	{"Black box", "Scheduler"},             // BlackBoxPage
	{"Processes", "Scheduler"},             // ProcessesPage
	{"System changes", "System"},           // ChangesPage
	{"Check", "Scheduler"},                 // CheckPage
	{"Graphics card", "Hardware"},          // GpuPage
	{"Drivers", "Hardware"},                // DriversPage
	{"Power", "Hardware"},                  // PowerPage
	{"Devices", "Hardware"},                // DevicesPage
	{"Settings", "Settings"},               // SettingsPage
	{"Updates and testing", "Settings"},    // UpdatePage
};

class Placeholder : public Page {
public:
	explicit Placeholder(PageId id) : id_(id) {}
	std::string Title() override { return L::T(kMeta[(size_t)id_].title); }
	const char* Section() override { return kMeta[(size_t)id_].section; }
	void Draw(ImRect area) override
	{
		ui::Box b(area.Min.x, area.Max.x);
		ui::Head(Title());
	}

private:
	PageId id_;
};

class SystemPlaceholder : public SystemPanelBase {
public:
	// Ui.Panel(body, padding 14,12,12,8) with the caption "This system".
	void Draw(ImRect area) override
	{
		ImGui::SetCursorScreenPos(area.Min);
		ui::Box b(area.Min.x, area.Max.x);
		ui::BeginPanel(ui::Thick(14, 12, 12, 8));
		ui::Head(L::T("This system"));
		ui::EndPanel();
	}
};

class HomePlaceholder : public HomeViewBase {
public:
	void Draw(ImRect area) override
	{
		ImGui::SetCursorScreenPos(area.Min + ui::S(24, 16));
		ui::Box b(area.Min.x + ui::S(24), area.Max.x - ui::S(24));
		ui::Head(L::T("Overview"));
	}
	SystemPanelBase* SysPanel() override { return &sys_; }

private:
	SystemPlaceholder sys_;
};

} // namespace

namespace pages {

void Register(PageId id, PageFactory f)
{
	if ((size_t)id < (size_t)PageId::Count) Reg().pages[(size_t)id] = f;
}

void RegisterHome(HomeFactory f) { Reg().home = f; }

void RegisterSystemPanel(SystemPanelFactory f) { Reg().sys = f; }

std::unique_ptr<Page> Make(PageId id)
{
	PageFactory f = (size_t)id < (size_t)PageId::Count ? Reg().pages[(size_t)id] : nullptr;
	if (f != nullptr)
	{
		std::unique_ptr<Page> p = f();
		if (p) return p;
	}
	return std::make_unique<Placeholder>(id);
}

std::unique_ptr<HomeViewBase> MakeHome()
{
	if (Reg().home != nullptr)
	{
		std::unique_ptr<HomeViewBase> h = Reg().home();
		if (h) return h;
	}
	return std::make_unique<HomePlaceholder>();
}

std::unique_ptr<SystemPanelBase> MakeSystemPanel()
{
	if (Reg().sys != nullptr)
	{
		std::unique_ptr<SystemPanelBase> s = Reg().sys();
		if (s) return s;
	}
	return std::make_unique<SystemPlaceholder>();
}

} // namespace pages
