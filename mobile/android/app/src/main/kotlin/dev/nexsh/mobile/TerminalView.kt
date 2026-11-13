package dev.nexsh.mobile

import android.content.Context
import android.graphics.Canvas
import android.graphics.Paint
import android.graphics.Typeface
import android.text.InputType
import android.util.TypedValue
import android.view.Choreographer
import android.view.GestureDetector
import android.view.KeyEvent
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.View
import android.view.inputmethod.BaseInputConnection
import android.view.inputmethod.EditorInfo
import android.view.inputmethod.InputConnection
import android.view.inputmethod.InputMethodManager
import kotlin.math.ceil
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

    private var frame: Frame? = null
    private var generation = 0L
    private var cellWidth = 1f
    private var cellHeight = 1f
    private var baseline = 0f
    private var scrollRemainder = 0f

    private val textPaint = Paint(Paint.ANTI_ALIAS_FLAG).apply { typeface = Typeface.MONOSPACE }
    private val fillPaint = Paint()
    private val run = StringBuilder()

    private val ticker = object : Choreographer.FrameCallback {
        override fun doFrame(frameTimeNanos: Long) {
            poll()
            Choreographer.getInstance().postFrameCallback(this)
        }
    }

    private val gestures = GestureDetector(context, object : GestureDetector.SimpleOnGestureListener() {
        override fun onDown(event: MotionEvent) = true

        override fun onSingleTapUp(event: MotionEvent): Boolean {
            showKeyboard()
            return true
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
        val frame = frame
        if (frame == null) {
            canvas.drawColor(DEFAULT_BACKGROUND)
            return
        }
        canvas.drawColor(frame.background)

        // 背景：只画和默认背景不同的单元格。
        for (index in 0 until frame.cellCount) {
            val color = frame.bg(index)
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
            val attrs = frame.attrs(index)
            val joins = char < 0x80 && run.isNotEmpty() && row == runRow &&
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
    }

    private fun drawText(canvas: Canvas, text: CharSequence, row: Int, col: Int, color: Int, attrs: Int) {
        textPaint.color = color
        textPaint.isFakeBoldText = attrs and Frame.BOLD != 0
        textPaint.textSkewX = if (attrs and Frame.ITALIC != 0) -0.2f else 0f
        textPaint.isUnderlineText = attrs and Frame.UNDERLINE != 0
        textPaint.isStrikeThruText = attrs and Frame.STRIKEOUT != 0
        canvas.drawText(text, 0, text.length, col * cellWidth, row * cellHeight + baseline, textPaint)
    }

    override fun onTouchEvent(event: MotionEvent): Boolean {
        pinch.onTouchEvent(event)
        if (!pinch.isInProgress) gestures.onTouchEvent(event)
        return true
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
    }
}
