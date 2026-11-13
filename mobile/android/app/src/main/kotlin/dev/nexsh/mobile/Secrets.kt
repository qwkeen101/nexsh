package dev.nexsh.mobile

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Base64
import java.security.KeyStore
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * 已保存的密码、私钥和私钥口令。密文放在应用私有的存储里，加密密钥由系统密钥库生成并留在里面：
 * 拿到这个文件的人没有密钥，密钥也导不出这台设备。
 */
class Secrets(context: Context) {
    private val prefs = context.getSharedPreferences("secrets", Context.MODE_PRIVATE)

    private fun key(): SecretKey {
        val store = KeyStore.getInstance(PROVIDER).apply { load(null) }
        (store.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val spec = KeyGenParameterSpec.Builder(
            ALIAS,
            KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT,
        )
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .build()
        return KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, PROVIDER).run {
            init(spec)
            generateKey()
        }
    }

    /** 没有保存过、或者解不开（密钥库被重置）时返回 null。 */
    fun load(account: String): String? {
        val encoded = prefs.getString(account, null) ?: return null
        return runCatching {
            val raw = Base64.decode(encoded, Base64.NO_WRAP)
            val cipher = Cipher.getInstance(TRANSFORMATION)
            cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, raw, 0, IV_SIZE))
            String(cipher.doFinal(raw, IV_SIZE, raw.size - IV_SIZE), Charsets.UTF_8)
        }.getOrNull()
    }

    /** 返回是否存成功了。 */
    fun store(account: String, secret: String): Boolean = runCatching {
        val cipher = Cipher.getInstance(TRANSFORMATION)
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val raw = cipher.iv + cipher.doFinal(secret.toByteArray(Charsets.UTF_8))
        prefs.edit().putString(account, Base64.encodeToString(raw, Base64.NO_WRAP)).apply()
    }.isSuccess

    fun delete(account: String) {
        prefs.edit().remove(account).apply()
    }

    /** 主机改名时把它的凭据带过去。 */
    fun renameHost(from: String, to: String) {
        val edit = prefs.edit()
        for (kind in KINDS) {
            val encoded = prefs.getString(kind + from, null) ?: continue
            edit.remove(kind + from).putString(kind + to, encoded)
        }
        edit.apply()
    }

    /** 主机删除后，它的凭据也不该留着。 */
    fun deleteHost(host: String) {
        val edit = prefs.edit()
        for (kind in KINDS) edit.remove(kind + host)
        edit.apply()
    }

    companion object {
        private const val PROVIDER = "AndroidKeyStore"
        private const val ALIAS = "nexsh-secrets"
        private const val TRANSFORMATION = "AES/GCM/NoPadding"
        private const val IV_SIZE = 12

        private val KINDS = listOf("password:", "key:", "passphrase:")

        fun password(host: String) = "password:" + host

        /** 私钥内容。 */
        fun key(host: String) = "key:" + host

        /** 私钥的口令。 */
        fun passphrase(host: String) = "passphrase:" + host
    }
}
