package dev.nexsh.mobile

/**
 * Rust 核心的入口（crates/nexsh-mobile）。SSH、终端模拟、主机列表的读写和校验都在那一侧，
 * 这里只有函数声明。会话用编号指代；编号 0 表示没有会话。
 */
object Bridge {
    init {
        System.loadLibrary("nexsh_mobile")
    }

    /** 修饰键位，和 Rust 一侧的约定一致。 */
    const val CTRL = 1
    const val ALT = 2
    const val SHIFT = 4

    /** 应用启动时调用一次。返回空字符串表示成功，否则是错误信息。 */
    @JvmStatic external fun start(dir: String): String

    /** 全部主机，JSON 数组。 */
    @JvmStatic external fun hosts(): String

    /** 保存表单；original 是被编辑主机原来的名称，新建时为 null。返回 JSON，见 HostForm。 */
    @JvmStatic external fun saveHost(form: String, original: String?): String

    /** 返回空字符串表示成功。 */
    @JvmStatic external fun deleteHost(name: String): String

    @JvmStatic external fun open(name: String, cols: Int, rows: Int, password: String?, passphrase: String?): Long

    @JvmStatic external fun reconnect(id: Long, password: String?, passphrase: String?)

    /** 画面自 since 那一代以来没变时返回 null。格式见 Frame。 */
    @JvmStatic external fun snapshot(id: Long, since: Long): ByteArray?

    /** 功能键，如 "enter"、"up"、"f5"。 */
    @JvmStatic external fun key(id: Long, name: String, modifiers: Int): Boolean

    @JvmStatic external fun text(id: Long, text: String, modifiers: Int)

    @JvmStatic external fun paste(id: Long, text: String)

    @JvmStatic external fun resize(id: Long, cols: Int, rows: Int)

    /** 正数向上翻看历史。 */
    @JvmStatic external fun scroll(id: Long, lines: Int)

    @JvmStatic external fun close(id: Long)
}
