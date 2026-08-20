# 交叉编到 Intel Mac 时用的 CMake 工具链文件。
#
# 为什么需要它：ct2rs 0.10 的 build.rs 用 cfg!(target_arch = "aarch64") 判断架构。
# 构建脚本是在**宿主**上编译运行的，这个 cfg 反映的是宿主而不是目标——在
# Apple Silicon 上交叉编 x86_64 时，它会强行传 -DCMAKE_OSX_ARCHITECTURES=arm64，
# 而 cc 传下去的又是 --target=x86_64-apple-macosx，两者打架，ruy 的 AVX 代码路径
# 直接报 unsupported option '-mavx'。是上游的判断错误，不是我们的代码。
#
# 工具链文件在初始缓存之后才被读取，所以这里 FORCE 覆盖得掉命令行的 -D。
set(CMAKE_SYSTEM_NAME Darwin)
set(CMAKE_SYSTEM_PROCESSOR x86_64)
set(CMAKE_OSX_ARCHITECTURES "x86_64" CACHE STRING "交叉编译目标架构" FORCE)
