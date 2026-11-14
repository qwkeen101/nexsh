package dev.nexsh.mobile

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.text.InputType
import android.util.TypedValue
import android.view.Choreographer
import android.view.GestureDetector
import android.view.HapticFeedbackConstants
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.ViewConfiguration
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import kotlin.math.ceil
import kotlin.math.hypot
import kotlin.math.max

/**
 * 终端画面。每个屏幕刷新周期向 Rust 核心要一次快照：画面没变时什么都拿不到，也就不重绘。
 *
 * 用传统 View 而不是 Compose 画布，是为了直接实现 InputConnection：
 * 输入法（包括中文输入法的候选和组合）的文字原样进终端，不经过文本框再转一手。
 */
class TerminalView(context: Context) : View(context) {
    /** 当前显示的会话；0 表示没有。 */
    var sessionId: Long = 0
        set(value) {
            field = value
            generation = 0
            applyGrid()
        }

    /** 粘滞修饰键的当前状态，以及用掉一次之后的回调。由按键栏提供。 */
    var modifiers: () -> Int = { 0 }
    var onModifiersUsed: () -> Unit = {}

    /** 状态、标题、延迟这些画面之外的信息变化时通知界面。 */
    var onFrame: (Frame) -> Unit = {}

    /** 选区出现或消失时通知界面，界面据此显示"复制"等操作。 */
    var onSelection: (Boolean) -> Unit = {}

    private var selecting = false

    /** 正在拖动选区的哪一端；NO_END 表示没有在拖。 */
    private var draggedEnd = NO_END
    private var dragX = 0f
    private var dragY = 0f
    private var dragFrames = 0

    /** 手指位置到它所指的单元格中心的偏移：手柄挂在文字下方，手指按着手柄时指的是上面那一格。 */
    private var grabX = 0f
    private var grabY = 0f

    /** 长按之后手指要移出这个距离才开始拖：按着不动时的轻微抖动不该改掉刚选中的词。 */
    private val touchSlop = ViewConfiguration.get(context).scaledTouchSlop
    private var dragArmed = false

    /** 触发了长按的那次触摸的按下时间，用来认出它后续的事件。 */
    private var longPressDownTime = 0L
    private val handleRadius = 9f * resources.displayMetrics.density
    private val handleReach = 28f * resources.displayMetrics.density

    private var frame: Frame? = null
    private var generation = 0L
    private var cellWidth = 1f
    private var cellHeight = 1f
    private var baseline = 0f
    private var scrollRemainder = 0f

    // 终端字体随应用带着，不用系统的"等宽字体"：不少手机的主题和字体替换会把它换成比例字体，
    // 那样每个字宽度不一，按格子排出来的画面全是散的。读不到时才退回系统的。
    private val regularFace = loadFace("fonts/JetBrainsMonoNL-Regular.ttf")
    private val boldFace = loadFace("fonts/JetBrainsMonoNL-Bold.ttf")
    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { typeface = regularFace ?: Typeface.MONOSPACE }

    /** 当前字体是不是真的等宽。不是的话（退回了系统字体）每个字符都单独按列定位。 */
    private var monospaced = true

    private fun loadFace(path: String): Typeface? =
        runCatching { Typeface.createFromAsset(context.assets, path) }.getOrNull()
    private val fillPaint = Paint()
    private val run = StringBuilder()

    private val ticker = object : Choreographer.FrameCallback {
        override fun doFrame(frameTimeNanos: Long) {
            autoScrollSelection()
            poll()
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(event: MotionEvent) = true

        // 有选区时点一下是取消选区，否则是唤出键盘。
        override fun onSingleTapUp(event: MotionEvent): Boolean {
            if (selecting) Bridge.clearSelection(sessionId) else showKeyboard()
            return true
        }

        // 长按选中手指下的词；不抬手接着拖，就是从这个词往后选。
        override fun onLongPress(event: MotionEvent) {
            if (sessionId == 0L) return
            performHapticFeedback(HapticFeedbackConstants.LONG_PRESS)
            Bridge.selectWord(sessionId, rowAt(event.y), colAt(event.x))
            draggedEnd = Bridge.SELECTION_END
            longPressDownTime = event.downTime
            dragX = event.x
            dragY = event.y
            grabX = 0f
            grabY = 0f
            dragArmed = false
        }

        // 手指往下拖是翻看更早的输出。不足一行的位移先攒着。
        override fun onScroll(first: MotionEvent?, current: MotionEvent, dx: Float, dy: Float): Boolean {
            scrollRemainder -= dy
            val lines = (scrollRemainder / cellHeight).toInt()
            if (lines != 0 && sessionId != 0L) {
                scrollRemainder -= lines * cellHeight
                Bridge.scroll(sessionId, lines)
            }
            return true
        }
    })

    private val pinch = ScaleGestureDetector(context, object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
        override fun onScale(detector: ScaleGestureDetector): Boolean {
            setFontSize(fontSizeSp * detector.scaleFactor)
            return true
        }
    })

    private var fontSizeSp = 13f

    init {
        isFocusable = true
        isFocusableInTouchMode = true
        setFontSize(fontSizeSp)
    }

    private fun setFontSize(sizeSp: Float) {
        fontSizeSp = sizeSp.coerceIn(8f, 28f)
        textPaint.textSize =
            TypedValue.applyDimension(TypedValue.COMPLEX_UNIT_SP, fontSizeSp, resources.displayMetrics)
        val metrics = textPaint.fontMetrics
        cellWidth = max(1f, textPaint.measureText("M"))
        monospaced = textPaint.measureText("i") == cellWidth && textPaint.measureText("iiii") == cellWidth * 4
        cellHeight = max(1f, ceil(metrics.descent - metrics.ascent))
        baseline = -metrics.ascent
        applyGrid()
        invalidate()
    }

    /** 按控件大小和字号算出行列数，告诉远端。 */
    private fun applyGrid() {
        if (sessionId == 0L || width == 0 || height == 0) return
        val cols = max(2, (width / cellWidth).toInt())
        val rows = max(1, (height / cellHeight).toInt())
        Bridge.resize(sessionId, cols, rows)
    }

    private fun poll() {
        if (sessionId == 0L) return
        val bytes = Bridge.snapshot(sessionId, generation) ?: return
        val next = Frame(bytes)
        generation = next.generation
        frame = next
        if (next.hasSelection != selecting) {
            selecting = next.hasSelection
            if (!selecting) draggedEnd = NO_END
            onSelection(selecting)
        }
        onFrame(next)
        invalidate()
    }

    override fun onAttachedToWindow() {
        super.onAttachedToWindow()
        Choreographer.getInstance().postFrameCallback(ticker)
    }

    override fun onDetachedFromWindow() {
        Choreographer.getInstance().removeFrameCallback(ticker)
        super.onDetachedFromWindow()
    }

    override fun onSizeChanged(width: Int, height: Int, oldWidth: Int, oldHeight: Int) {
        super.onSizeChanged(width, height, oldWidth, oldHeight)
        applyGrid()
    }

    override fun onDraw(canvas: Canvas) {
        // 嵌在 Compose 里的控件拿到的画布不按自己的边界裁剪；不裁的话下面的整屏填色会盖掉
        // 终端上方的标题栏和提示条。
        canvas.clipRect(0, 0, width, height)
        val frame = frame
        if (frame == null) {
            canvas.drawColor(DEFAULT_BACKGROUND)
            return
        }
        canvas.drawColor(frame.background)

        // 背景：只画和默认背景不同的单元格。选中的单元格换成选区的底色。
        for (index in 0 until frame.cellCount) {
            val color = if (frame.attrs(index) and Frame.SELECTED != 0) SELECTION_COLOR else frame.bg(index)
            if (color == frame.background) continue
            val left = frame.col(index) * cellWidth
            val top = frame.row(index) * cellHeight
            val span = if (frame.attrs(index) and Frame.WIDE != 0) 2 else 1
            fillPaint.color = color
            canvas.drawRect(left, top, left + cellWidth * span, top + cellHeight, fillPaint)
        }

        // 文字：同一行里样式相同、列连续的 ASCII 字符攒成一段一起画；
        // 其他字符（中文、符号）可能来自别的字体、宽度不等于一格，逐个按列定位。
        var runRow = -1
        var runCol = 0
        var runColor = 0
        var runAttrs = 0
        fun flush() {
            if (run.isEmpty()) return
            drawText(canvas, run, runRow, runCol, runColor, runAttrs)
            run.setLength(0)
        }
        for (index in 0 until frame.cellCount) {
            val char = frame.char(index)
            if (char == 0 || char == ' '.code) continue
            val row = frame.row(index)
            val col = frame.col(index)
            val color = frame.fg(index)
            // 选中与否不影响文字怎么画，去掉这一位，免得选区边界把一段文字拆开。
            val attrs = frame.attrs(index) and Frame.SELECTED.inv()
            val joins = monospaced && char < 0x80 && run.isNotEmpty() && row == runRow &&
                col == runCol + run.length && color == runColor && attrs == runAttrs
            if (!joins) {
                flush()
                runRow = row
                runCol = col
                runColor = color
                runAttrs = attrs
            }
            run.appendCodePoint(char)
            if (char >= 0x80) flush()
        }
        flush()

        if (frame.cursorRow >= 0) {
            val left = frame.cursorCol * cellWidth
            val top = frame.cursorRow * cellHeight
            fillPaint.color = (frame.foreground and 0x00ffffff) or (if (hasFocus()) 0x99000000.toInt() else 0x44000000)
            canvas.drawRect(left, top, left + cellWidth, top + cellHeight, fillPaint)
        }

        // 选区两端的手柄：起点的挂在那一格的左下角，终点的挂在右下角。
        if (frame.hasSelection) {
            fillPaint.color = HANDLE_COLOR
            if (frame.selectionStartRow >= 0) {
                canvas.drawCircle(startHandleX(frame), handleY(frame.selectionStartRow), handleRadius, fillPaint)
            }
            if (frame.selectionEndRow >= 0) {
                canvas.drawCircle(endHandleX(frame), handleY(frame.selectionEndRow), handleRadius, fillPaint)
            }
        }
    }

    // ---- 选区 ----

    private fun rowAt(y: Float): Int = (y / cellHeight).toInt().coerceAtLeast(0)
    private fun colAt(x: Float): Int = (x / cellWidth).toInt().coerceAtLeast(0)

    private fun startHandleX(frame: Frame) = frame.selectionStartCol * cellWidth
    private fun endHandleX(frame: Frame) = (frame.selectionEndCol + 1) * cellWidth
    private fun handleY(row: Int) = (row + 1) * cellHeight + handleRadius

    /** 手指落在哪个手柄上；都不在时是 NO_END。两个手柄挨得近时取离得更近的那个。 */
    private fun handleAt(x: Float, y: Float): Int {
        val frame = frame ?: return NO_END
        if (!frame.hasSelection) return NO_END
        val toStart = if (frame.selectionStartRow < 0) Float.MAX_VALUE else {
            hypot(x - startHandleX(frame), y - handleY(frame.selectionStartRow))
        }
        val toEnd = if (frame.selectionEndRow < 0) Float.MAX_VALUE else {
            hypot(x - endHandleX(frame), y - handleY(frame.selectionEndRow))
        }
        return when {
            toStart > handleReach && toEnd > handleReach -> NO_END
            toStart < toEnd -> Bridge.SELECTION_START
            else -> Bridge.SELECTION_END
        }
    }

    /** 把正在拖的那一端移到手指所指的单元格。 */
    private fun dragSelection() {
        if (draggedEnd == NO_END || !dragArmed || sessionId == 0L) return
        Bridge.moveSelection(sessionId, draggedEnd, rowAt(dragY + grabY), colAt(dragX + grabX))
    }

    /** 开始拖一个手柄：记下手指和这一端所在单元格中心的偏移，拖动时一直保持。 */
    private fun grabHandle(end: Int, x: Float, y: Float) {
        val frame = frame ?: return
        val start = end == Bridge.SELECTION_START
        val row = if (start) frame.selectionStartRow else frame.selectionEndRow
        val col = if (start) frame.selectionStartCol else frame.selectionEndCol
        grabX = (col + 0.5f) * cellWidth - x
        grabY = (row + 0.5f) * cellHeight - y
        dragX = x
        dragY = y
        dragArmed = true
    }

    /** 拖着选区停在控件的上下边缘时，画面跟着滚，每 4 帧滚一行。 */
    private fun autoScrollSelection() {
        if (draggedEnd == NO_END || sessionId == 0L) return
        if (!dragArmed) return
        val lines = when {
            dragY + grabY < cellHeight -> 1
            dragY + grabY > height - cellHeight -> -1
            else -> return
        }
        dragFrames += 1
        if (dragFrames % 4 != 0) return
        Bridge.scroll(sessionId, lines)
        dragSelection()
    }

    private fun drawText(canvas: Canvas, text: CharSequence, row: Int, col: Int, color: Int, attrs: Int) {
        textPaint.color = color
        val bold = attrs and Frame.BOLD != 0
        // 有真正的粗体字形就用它；没有时让系统描粗。
        textPaint.typeface = (if (bold) boldFace else null) ?: regularFace ?: Typeface.MONOSPACE
        textPaint.isFakeBoldText = bold && boldFace == null
        textPaint.textSkewX = if (attrs and Frame.ITALIC != 0) -0.2f else 0f
        textPaint.isUnderlineText = attrs and Frame.UNDERLINE != 0
        textPaint.isStrikeThruText = attrs and Frame.STRIKEOUT != 0
        canvas.drawText(text, 0, text.length, col * cellWidth, row * cellHeight + baseline, textPaint)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                draggedEnd = handleAt(event.x, event.y)
                if (draggedEnd != NO_END) {
                    // 按在手柄上：这次触摸只用来拖选区，不交给滚动和长按。
                    grabHandle(draggedEnd, event.x, event.y)
                    parent?.requestDisallowInterceptTouchEvent(true)
                    return true
                }
            }
            MotionEvent.ACTION_MOVE -> if (draggedEnd != NO_END) {
                if (!dragArmed && hypot(event.x - dragX, event.y - dragY) > touchSlop) dragArmed = true
                if (dragArmed) {
                    dragX = event.x
                    dragY = event.y
                    dragSelection()
                }
                return true
            }
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> if (draggedEnd != NO_END) {
                draggedEnd = NO_END
                // 长按之后的抬手还要让手势识别器收尾，手柄拖动的不用。
                if (event.downTime != longPressDownTime) return true
            }
        }
        if (event.actionMasked == MotionEvent.ACTION_DOWN) longPressDownTime = 0L
        pinch.onTouchEvent(event)
        if (!pinch.isInProgress) gestures.onTouchEvent(event)
        return true
    }

    /** 选中的文字；没有选区时为 null。 */
    fun selectionText(): String? = if (sessionId == 0L) null else Bridge.selectionText(sessionId)

    fun selectAll() {
        if (sessionId != 0L) Bridge.selectAll(sessionId)
    }

    fun clearSelection() {
        if (sessionId != 0L) Bridge.clearSelection(sessionId)
    }

    fun showKeyboard() {
        requestFocus()
        val manager = context.getSystemService(Context.INPUT_METHOD_SERVICE) as InputMethodManager
        manager.showSoftInput(this, 0)
    }

    // ---- 输入 ----

    /** 输入法提交的文字，以及按键栏上的符号键。 */
    fun sendText(text: String) {
        if (text.isEmpty() || sessionId == 0L) return
        Bridge.text(sessionId, text, modifiers())
        onModifiersUsed()
    }

    /** 按键栏和实体键盘上的功能键。 */
    fun sendKey(name: String, extraModifiers: Int = 0) {
        if (sessionId == 0L) return
        Bridge.key(sessionId, name, modifiers() or extraModifiers)
        onModifiersUsed()
    }

    fun paste(text: String) {
        if (sessionId != 0L) Bridge.paste(sessionId, text)
    }

    override fun onCheckIsTextEditor() = true

    override fun onCreateInputConnection(attributes: EditorInfo): InputConnection {
        // 不要联想和自动更正：它们会在提交时改写已经发给远端的字符。
        attributes.inputType = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        attributes.imeOptions = EditorInfo.IME_FLAG_NO_FULLSCREEN or
            EditorInfo.IME_FLAG_NO_EXTRACT_UI or EditorInfo.IME_ACTION_NONE
        return object : BaseInputConnection(this, false) {
            /** 输入法正在组合、还没提交的文字（拼音等）。组合期间不发给远端。 */
            private var composing = ""

            override fun setComposingText(text: CharSequence, newCursorPosition: Int): Boolean {
                composing = text.toString()
                return true
            }

            override fun finishComposingText(): Boolean {
                val pending = composing
                composing = ""
                sendText(pending)
                return true
            }

            override fun commitText(text: CharSequence, newCursorPosition: Int): Boolean {
                composing = ""
                // 有些输入法把回车当作文字提交。
                if (text.toString() == "\n") sendKey("enter") else sendText(text.toString())
                return true
            }

            override fun deleteSurroundingText(beforeLength: Int, afterLength: Int): Boolean {
                repeat(max(1, beforeLength)) { sendKey("backspace") }
                return true
            }

            override fun sendKeyEvent(event: KeyEvent): Boolean {
                if (event.action == KeyEvent.ACTION_DOWN) handleKey(event)
                return true
            }
        }
    }

    override fun onKeyDown(keyCode: Int, event: KeyEvent): Boolean =
        handleKey(event) || super.onKeyDown(keyCode, event)

    private fun handleKey(event: KeyEvent): Boolean {
        var held = 0
        if (event.isCtrlPressed) held = held or Bridge.CTRL
        if (event.isAltPressed) held = held or Bridge.ALT
        if (event.isShiftPressed) held = held or Bridge.SHIFT
        val name = when (event.keyCode) {
            KeyEvent.KEYCODE_ENTER, KeyEvent.KEYCODE_NUMPAD_ENTER -> "enter"
            KeyEvent.KEYCODE_DEL -> "backspace"
            KeyEvent.KEYCODE_FORWARD_DEL -> "delete"
            KeyEvent.KEYCODE_TAB -> "tab"
            KeyEvent.KEYCODE_ESCAPE -> "escape"
            KeyEvent.KEYCODE_DPAD_UP -> "up"
            KeyEvent.KEYCODE_DPAD_DOWN -> "down"
            KeyEvent.KEYCODE_DPAD_LEFT -> "left"
            KeyEvent.KEYCODE_DPAD_RIGHT -> "right"
            KeyEvent.KEYCODE_MOVE_HOME -> "home"
            KeyEvent.KEYCODE_MOVE_END -> "end"
            KeyEvent.KEYCODE_PAGE_UP -> "pageup"
            KeyEvent.KEYCODE_PAGE_DOWN -> "pagedown"
            KeyEvent.KEYCODE_INSERT -> "insert"
            in KeyEvent.KEYCODE_F1..KeyEvent.KEYCODE_F12 -> "f" + (event.keyCode - KeyEvent.KEYCODE_F1 + 1)
            else -> null
        }
        if (name != null) {
            sendKey(name, held)
            return true
        }
        // 可打印字符。取不带 Ctrl/Alt 的字符，修饰键交给核心去编码（Ctrl+C 变成 0x03 等）。
        val char = event.getUnicodeChar(event.metaState and (KeyEvent.META_SHIFT_MASK or KeyEvent.META_CAPS_LOCK_ON))
        if (char == 0 || sessionId == 0L) return false
        Bridge.text(sessionId, String(Character.toChars(char)), modifiers() or (held and Bridge.SHIFT.inv()))
        onModifiersUsed()
        return true
    }

    companion object {
        private const val DEFAULT_BACKGROUND = 0xff16161a.toInt()
        private const val SELECTION_COLOR = 0xff2f4a7a.toInt()
        private const val HANDLE_COLOR = 0xff6c9cf5.toInt()
        private const val NO_END = -1
    }
}
