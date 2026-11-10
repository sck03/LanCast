#pragma once
#include <algorithm>
#include <cstdint>
#include <windows.h>

// Translate wheel input to pixels while retaining sub-pixel movement. The caller
// supplies the system preference so this policy is independent of global settings.
class WheelScroll {
  public:
    int consume(int delta, UINT lines, int line_height, int page_height) {
        const auto step = lines == WHEEL_PAGESCROLL
                              ? static_cast<int64_t>(page_height)
                              : std::min(static_cast<int64_t>(lines) * line_height,
                                         static_cast<int64_t>(page_height));
        if (step <= 0) {
            remainder_ = 0;
            return 0;
        }
        const auto distance = remainder_ + delta * step;
        remainder_ = distance % WHEEL_DELTA;
        return static_cast<int>(distance / WHEEL_DELTA);
    }

  private:
    int64_t remainder_ = 0;
};
