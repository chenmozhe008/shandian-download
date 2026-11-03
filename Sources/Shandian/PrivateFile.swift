import Darwin
import Foundation

/// 在目标目录先创建仅所有者可读写的临时文件，再原子替换，避免凭证或链接短暂暴露。
enum PrivateFile {
    /// 新建日志时使用私有权限；已有文件仅追加，不擅自修改其权限。
    static func openForAppending(_ url: URL) throws -> FileHandle {
        let fd = open(url.path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
        guard fd >= 0 else { throw posixError() }
        return FileHandle(fileDescriptor: fd, closeOnDealloc: true)
    }

    static func write(_ data: Data, to url: URL) throws {
        let temporary = url.deletingLastPathComponent().appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString).tmp")
        let fd = open(temporary.path, O_WRONLY | O_CREAT | O_EXCL, 0o600)
        guard fd >= 0 else { throw posixError() }
        defer { close(fd); unlink(temporary.path) }
        let handle = FileHandle(fileDescriptor: fd, closeOnDealloc: false)
        try handle.write(contentsOf: data)
        guard fsync(fd) == 0 else { throw posixError() }
        guard rename(temporary.path, url.path) == 0 else { throw posixError() }
    }

    private static func posixError() -> POSIXError {
        POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
    }
}
