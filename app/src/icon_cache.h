// IconCache.cs: the icons of program files (Icon.ExtractAssociatedIcon), read off the UI thread and kept for the
// whole run (a failed one is remembered as none). Owner: app shell.
#pragma once
#include <string>

#include "imgui.h"

namespace icons {

/// The icon of `path` at `pixels` px as an ImGui texture, or 0 while it is read / when there is none.
/// The window is redrawn when a read finishes.
ImTextureID Get(const std::string& path, int pixels);
/// Draws the icon (nothing while it is not there) into a square at pos.
void Draw(const std::string& path, int pixels, ImVec2 pos, float sizePx);

} // namespace icons
