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

    const val SELECTION_START = 0
    const val SELECTION_END = 1

    /** 应用启动时调用一次。返回空字符串表示成功，否则是错误信息。 */
    @JvmStatic external fun start(dir: String): String

    /** 全部主机，JSON 数组。 */
    @JvmStatic external fun hosts(): String

    /** 保存表单；original 是被编辑主机原来的名称，新建时为 null。返回 JSON，见 HostForm。 */
    @JvmStatic external fun saveHost(form: String, original: String?): String

    /** 返回空字符串表示成功。 */
    @JvmStatic external fun deleteHost(name: String): String

    /**
     * 检查一段文字是不是私钥；passphrase 没有就传 null。返回 JSON：ok、encrypted，能解开时还有
     * algorithm 和 fingerprint；不能用时 ok 为 false，field 是 "key" 或 "passphrase"，message 是原因。
     */
    @JvmStatic external fun inspectKey(text: String, passphrase: String?): String

    /** 密码、口令、私钥内容没有就传 null。它们留在会话的内存里，断线重连时接着用。 */
    @JvmStatic external fun open(
        name: String,
        cols: Int,
        rows: Int,
        password: String?,
        passphrase: String?,
        key: String?,
    ): Long

    /** 用户发起的重新连接。传 null 的沿用会话记着的。 */
    @JvmStatic external fun reconnect(id: Long, password: String?, passphrase: String?)

    /** 正在等待自动重连的会话马上重连。 */
    @JvmStatic external fun retryNow(id: Long)

    /** 停止自动重连。 */
    @JvmStatic external fun stopRetry(id: Long)

    /** 网络恢复或应用回到前台时调用：等着重连的会话都马上试一次。 */
    @JvmStatic external fun networkAvailable()

    /** 画面自 since 那一代以来没变时返回 null。格式见 Frame。 */
    @JvmStatic external fun snapshot(id: Long, since: Long): ByteArray?

    /** 功能键，如 "enter"、"up"、"f5"。 */
    @JvmStatic external fun key(id: Long, name: String, modifiers: Int): Boolean

    @JvmStatic external fun text(id: Long, text: String, modifiers: Int)

    @JvmStatic external fun paste(id: Long, text: String)

    @JvmStatic external fun resize(id: Long, cols: Int, rows: Int)

    /** 正数向上翻看历史。 */
    @JvmStatic external fun scroll(id: Long, lines: Int)

    /** 选中可见区域里这个位置上的词。 */
    @JvmStatic external fun selectWord(id: Long, row: Int, col: Int)

    /** 把选区的一端拖到这个位置；end 是 SELECTION_START 或 SELECTION_END。 */
    @JvmStatic external fun moveSelection(id: Long, end: Int, row: Int, col: Int)

    /** 选中全部内容，包括滚出屏幕的历史。 */
    @JvmStatic external fun selectAll(id: Long)

    @JvmStatic external fun clearSelection(id: Long)

    /** 选中的文字；没有选区时为 null。 */
    @JvmStatic external fun selectionText(id: Long): String?

    @JvmStatic external fun close(id: Long)
}
