package dev.nexsh.mobile

import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.ui.graphics.Color

/** 和桌面端同一套分层配色：内容区最深，栏和卡片逐层变亮。 */
object Palette {
    val Background = Color(0xFF16161A)
    val Bar = Color(0xFF1C1C21)
    val Surface = Color(0xFF232329)
    val Border = Color(0xFF2C2C33)
    val Selected = Color(0xFF34343C)
    val Text = Color(0xFFE4E4E8)
    val Muted = Color(0xFF9A9AA4)
    val Faint = Color(0xFF6B6B76)
    val Accent = Color(0xFF6C9CF5)
    val Success = Color(0xFF9ECE6A)
    val Notice = Color(0xFFE0AF68)
    val Error = Color(0xFFF7768E)

    /** 头像底色，按主机名固定选一种。 */
    val Avatars = listOf(
        Color(0xFF7AA2F7), Color(0xFF9ECE6A), Color(0xFFE0AF68), Color(0xFFF7768E),
        Color(0xFFBB9AF7), Color(0xFF7DCFFF), Color(0xFFFF9E64), Color(0xFF73DACA),
    )
}

@Composable
fun NexshTheme(content: @Composable () -> Unit) {
    MaterialTheme(
        colorScheme = darkColorScheme(
            primary = Palette.Accent,
            onPrimary = Color(0xFF0B1220),
            background = Palette.Background,
            onBackground = Palette.Text,
            surface = Palette.Bar,
            onSurface = Palette.Text,
            surfaceVariant = Palette.Surface,
            onSurfaceVariant = Palette.Muted,
            surfaceContainer = Palette.Surface,
            surfaceContainerHigh = Palette.Surface,
            surfaceContainerHighest = Palette.Selected,
            outline = Palette.Border,
            outlineVariant = Palette.Border,
            error = Palette.Error,
        ),
        content = content,
    )
}
