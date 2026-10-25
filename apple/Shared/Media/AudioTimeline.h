#pragma once
#include <algorithm>
#include <cmath>
#include <cstdint>
#include <optional>

// Source presentation time expressed in the sender ADM's 48 kHz sample clock.
// Invalid, repeated or backwards source times are dropped; gaps remain gaps.
class LCAudioTimeline {
    std::optional<double> origin_, previous_;
    double next_ = 0;
    bool gap_ = false;
public:
    void reset() { origin_.reset(); previous_.reset(); next_ = 0; gap_ = false; }
    bool begin(double seconds) {
        if (!std::isfinite(seconds) || seconds < 0 || (previous_ && seconds <= *previous_))
            return false;
        const double origin = origin_.value_or(seconds);
        const double mapped = std::round((seconds - origin) * 48000.0);
        if (!std::isfinite(mapped) || mapped < 0 || mapped > 9007199254740991.0)
            return false;
        gap_ = mapped > next_ + 1;
        next_ = std::max(next_, mapped);
        origin_ = origin; previous_ = seconds;
        return true;
    }
    bool hasGap() const { return gap_; }
    double take(uint32_t frames) { const double start = next_; next_ += frames; return start; }
};
