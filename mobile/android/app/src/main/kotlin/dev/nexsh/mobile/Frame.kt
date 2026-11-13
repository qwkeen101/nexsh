package dev.nexsh.mobile

import java.nio.ByteBuffer
import java.nio.ByteOrder

/**
 * 一帧画面快照。字节布局由 Rust 一侧的 wire 模块定义；这里按偏移直接读，不把单元格拆成对象。
 */
class Frame(private val bytes: ByteArray) {
    private val buffer = ByteBuffer.wrap(bytes).order(ByteOrder.LITTLE_ENDIAN)

    val state: Int = bytes[1].toInt()
    private val modes: Int = bytes[2].toInt()
    val generation: Long = buffer.getLong(4)
    val cols: Int = u16(12)
    val rows: Int = u16(14)
    val cursorRow: Int = buffer.getShort(16).toInt()
    val cursorCol: Int = buffer.getShort(18).toInt()
    val background: Int = buffer.getInt(20) or OPAQUE
    val foreground: Int = buffer.getInt(24) or OPAQUE

    /** 没有测到时为 -1。 */
    val latencyMs: Int = buffer.getInt(28)
    /** 选区两端在可见区域里的位置；没有选区或那一端滚出去了是 -1。 */
    val selectionStartRow: Int = buffer.getShort(32).toInt()
    val selectionStartCol: Int = buffer.getShort(34).toInt()
    val selectionEndRow: Int = buffer.getShort(36).toInt()
    val selectionEndCol: Int = buffer.getShort(38).toInt()
    val title: String
    val message: String
    val cellCount: Int
    private val cellsAt: Int

    init {
        var at = 40
        val titleLength = u16(at)
        title = String(bytes, at + 2, titleLength, Charsets.UTF_8)
        at += 2 + titleLength
        val messageLength = u16(at)
        message = String(bytes, at + 2, messageLength, Charsets.UTF_8)
        at += 2 + messageLength
        cellCount = buffer.getInt(at)
        cellsAt = at + 4
    }

    val bracketedPaste: Boolean get() = modes and 2 != 0
    val hasSelection: Boolean get() = modes and 8 != 0

    private fun u16(at: Int): Int = buffer.getShort(at).toInt() and 0xffff

    fun row(index: Int): Int = u16(cellsAt + index * CELL_SIZE)
    fun col(index: Int): Int = u16(cellsAt + index * CELL_SIZE + 2)
    fun char(index: Int): Int = buffer.getInt(cellsAt + index * CELL_SIZE + 4)
    fun fg(index: Int): Int = buffer.getInt(cellsAt + index * CELL_SIZE + 8) or OPAQUE
    fun bg(index: Int): Int = buffer.getInt(cellsAt + index * CELL_SIZE + 12) or OPAQUE
    fun attrs(index: Int): Int = bytes[cellsAt + index * CELL_SIZE + 16].toInt()

    companion object {
        private const val CELL_SIZE = 17
        private const val OPAQUE = 0xff000000.toInt()

        const val CONNECTING = 0
        const val CONNECTED = 1
        const val NEEDS_PASSWORD = 2
        const val NEEDS_PASSPHRASE = 3
        const val EXITED = 4
        const val FAILED = 5
        const val RECONNECTING = 6

        const val BOLD = 1
        const val ITALIC = 2
        const val UNDERLINE = 4
        const val STRIKEOUT = 8
        const val WIDE = 16
        const val SELECTED = 32
    }
}
