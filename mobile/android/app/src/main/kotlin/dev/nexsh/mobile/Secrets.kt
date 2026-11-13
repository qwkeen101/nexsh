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
 *
 * 私钥是一个带名字的库：导入一次，多台主机都可以选它。每台主机只记着它用的是哪一把。
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

    // ---- 私钥库 ----

    /** 一把保存着的私钥。summary 是导入时算出的算法和指纹，显示用，不是秘密。 */
    data class SavedKey(val name: String, val summary: String)

    fun keys(): List<SavedKey> = prefs.all.keys
        .filter { it.startsWith(KEY_FILE) }
        .map { it.removePrefix(KEY_FILE) }
        .sorted()
        .map { SavedKey(it, prefs.getString(KEY_INFO + it, null).orEmpty()) }

    /** 存一把私钥；口令为空表示不存口令（没有口令，或留到连接时再问）。返回是否存成功了。 */
    fun saveKey(name: String, content: String, summary: String, passphrase: String): Boolean {
        if (!store(KEY_FILE + name, content)) return false
        prefs.edit().putString(KEY_INFO + name, summary).remove(KEY_PASS + name).apply()
        if (passphrase.isNotEmpty()) store(KEY_PASS + name, passphrase)
        return true
    }

    fun keyContent(name: String): String? = load(KEY_FILE + name)

    fun keyPassphrase(name: String): String? = load(KEY_PASS + name)

    fun rememberKeyPassphrase(name: String, passphrase: String) {
        store(KEY_PASS + name, passphrase)
    }

    /** 用着这把私钥的主机。删除前给用户看一眼。 */
    fun hostsUsing(name: String): List<String> = prefs.all
        .filter { it.key.startsWith(HOST_KEY) && it.value == name }
        .map { it.key.removePrefix(HOST_KEY) }
        .sorted()

    /** 删掉一把私钥。用着它的主机不再指向它，下次连接时会提示重新选择。 */
    fun deleteKey(name: String) {
        val edit = prefs.edit().remove(KEY_FILE + name).remove(KEY_INFO + name).remove(KEY_PASS + name)
        for (host in hostsUsing(name)) edit.remove(HOST_KEY + host)
        edit.apply()
    }

    /** 这台主机用的是哪一把私钥；没选过、或那一把已经删了，为 null。 */
    fun hostKey(host: String): String? =
        prefs.getString(HOST_KEY + host, null)?.takeIf { prefs.contains(KEY_FILE + it) }

    fun setHostKey(host: String, key: String?) {
        val edit = prefs.edit()
        if (key == null) edit.remove(HOST_KEY + host) else edit.putString(HOST_KEY + host, key)
        edit.apply()
    }

    /**
     * 早期版本把私钥按主机各存一份。把它们搬进私钥库，以主机名命名，主机指向搬过去的那一把。
     * describe 给出私钥的描述（算法和指纹）。
     */
    fun migrateHostKeys(describe: (String) -> String?) {
        for (account in prefs.all.keys.filter { it.startsWith(OLD_HOST_KEY) }) {
            val host = account.removePrefix(OLD_HOST_KEY)
            val content = load(account)
            if (content != null) {
                var name = host
                var suffix = 2
                while (prefs.contains(KEY_FILE + name)) name = host + " " + suffix++
                val passphrase = load(OLD_HOST_PASS + host).orEmpty()
                if (saveKey(name, content, describe(content).orEmpty(), passphrase)) setHostKey(host, name)
            }
            prefs.edit().remove(account).remove(OLD_HOST_PASS + host).apply()
        }
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

        /** 按主机存的东西：密码（加密），以及它用的私钥的名字（明文，只是个名字）。 */
        private const val HOST_KEY = "hostkey:"
        private val KINDS = listOf("password:", HOST_KEY)

        /** 私钥库：内容（加密）、口令（加密）、描述（明文）。 */
        private const val KEY_FILE = "keyfile:"
        private const val KEY_PASS = "keypass:"
        private const val KEY_INFO = "keyinfo:"

        /** 早期版本按主机存的私钥和口令，见 migrateHostKeys。 */
        private const val OLD_HOST_KEY = "key:"
        private const val OLD_HOST_PASS = "passphrase:"

        fun password(host: String) = "password:" + host
    }
}
