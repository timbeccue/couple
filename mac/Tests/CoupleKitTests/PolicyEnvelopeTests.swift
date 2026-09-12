import Foundation
import Testing
@testable import CoupleKit

@Suite struct PolicyEnvelopeTests {
    @Test func malformedAndErrorEnvelopesHaveNoValue() {
        let envelopes: [String?] = [
            nil, "", "not JSON", "[]", "null",
            #"{"value":42}"#,
            #"{"ok":"true","value":42}"#,
            #"{"ok":false,"value":42,"error":{"kind":"invalid","message":"bad request"}}"#,
        ]
        for envelope in envelopes {
            #expect(PolicyEnvelope.value(fromJSON: envelope) == nil)
        }
    }

    @Test func missingAndNullSuccessValuesHaveNoValue() {
        #expect(PolicyEnvelope.value(fromJSON: #"{"ok":true}"#) == nil)
        #expect(PolicyEnvelope.value(fromJSON: #"{"ok":true,"value":null}"#) == nil)
    }

    @Test func successValuesKeepTheirJSONShape() throws {
        let object = try #require(PolicyEnvelope.value(
            fromJSON: #"{"ok":true,"value":{"selected":"audio","extra":null}}"#) as? [String: Any])
        #expect(object["selected"] as? String == "audio")
        #expect(object["extra"] is NSNull)

        let array = try #require(PolicyEnvelope.value(
            fromJSON: #"{"ok":true,"value":[1,2.5,null]}"#) as? [Any])
        #expect(array.count == 3)
        #expect((array[0] as? NSNumber)?.doubleValue == 1)
        #expect((array[1] as? NSNumber)?.doubleValue == 2.5)
        #expect(array[2] is NSNull)

        #expect((PolicyEnvelope.value(fromJSON: #"{"ok":true,"value":2.5}"#) as? NSNumber)?.doubleValue == 2.5)
        #expect(PolicyEnvelope.value(fromJSON: #"{"ok":true,"value":false}"#) as? Bool == false)
        #expect(PolicyEnvelope.value(fromJSON: #"{"ok":true,"value":"text"}"#) as? String == "text")
    }

    @Test func missingCoreStringsUseEveryWrapperFallback() {
        #expect(PolicyEnvelope.decodeValue(nil) == nil)
        #expect(PolicyEnvelope.decodeValueArray(nil) == nil)
        #expect(PolicyEnvelope.decodeScalar(nil) == nil)
    }
}
