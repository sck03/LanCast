cmake_minimum_required(VERSION 3.25)
project(LanCast LANGUAGES C CXX)
set(CMAKE_CXX_STANDARD 20)
set(CMAKE_CXX_STANDARD_REQUIRED ON)
enable_testing()
include(FetchContent)
FetchContent_Declare(json GIT_REPOSITORY https://github.com/nlohmann/json.git GIT_TAG 65ee68451d8eb2b5f3a30b410476ab83deb3289b)
FetchContent_MakeAvailable(json)
add_subdirectory(media)
add_executable(LanCast WIN32 src/main.cpp src/media.cpp)
add_dependencies(LanCast lancast_rtc)
target_include_directories(LanCast PRIVATE ../core/include ../media-native/include)
target_compile_definitions(LanCast PRIVATE UNICODE _UNICODE NOMINMAX WIN32_LEAN_AND_MEAN)
target_compile_options(LanCast PRIVATE /W4 /utf-8 /EHsc)
target_link_libraries(LanCast PRIVATE nlohmann_json::nlohmann_json
    "${CMAKE_CURRENT_SOURCE_DIR}/../target/x86_64-pc-windows-msvc/release/lancast_core.dll.lib"
    ole32 shell32 comdlg32 comctl32)
add_custom_command(TARGET LanCast POST_BUILD COMMAND ${CMAKE_COMMAND} -E copy_if_different
    "${CMAKE_CURRENT_SOURCE_DIR}/../target/x86_64-pc-windows-msvc/release/lancast_core.dll" "$<TARGET_FILE_DIR:LanCast>")
