package demo

class Money(val cents: Int) {
    operator fun plus(other: Money): Money = Money(cents + other.cents)

    override fun toString(): String = "$cents"

    fun kotlinUnusedHelper(): Int = 1
}
