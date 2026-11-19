#include "capture_pacing.h"
#include <cstdlib>

int main() {
    using namespace lancast;
    if (repeat_capture(false, false, 999999, 0))
        return EXIT_FAILURE;
    if (!repeat_capture(false, false, 1000000, 0))
        return EXIT_FAILURE;
    if (!repeat_capture(false, true, 10, 0))
        return EXIT_FAILURE;
    if (!repeat_capture(true, false, 10, 0))
        return EXIT_FAILURE;
    if (capture_delay_us(100, 33433, false) != 33000)
        return EXIT_FAILURE;
    if (capture_delay_us(100, 33433, true) != 10000)
        return EXIT_FAILURE;
    if (capture_delay_us(100, 50, false) != 1000)
        return EXIT_FAILURE;
    return EXIT_SUCCESS;
}
