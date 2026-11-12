module top(input logic [1:0] lsu_type_i, input logic handle_misaligned_q, input logic [1:0] data_offset, input logic [31:0] data_addr_i, output logic [3:0] be, output logic [7:0] strb);
logic [3:0] data_be;
  always_comb begin
    unique case (lsu_type_i) // Data type 00 Word, 01 Half word, 11,10 byte
      2'b00: begin // Writing a word
        if (!handle_misaligned_q) begin // first part of potentially misaligned transaction
          unique case (data_offset)
            2'b00:   data_be = 4'b1111;
            2'b01:   data_be = 4'b1110;
            2'b10:   data_be = 4'b1100;
            2'b11:   data_be = 4'b1000;
            default: data_be = 4'b1111;
          endcase // case (data_offset)
        end else begin // second part of misaligned transaction
          unique case (data_offset)
            2'b00:   data_be = 4'b0000; // this is not used, but included for completeness
            2'b01:   data_be = 4'b0001;
            2'b10:   data_be = 4'b0011;
            2'b11:   data_be = 4'b0111;
            default: data_be = 4'b1111;
          endcase // case (data_offset)
        end
      end

      2'b01: begin // Writing a half word
        if (!handle_misaligned_q) begin // first part of potentially misaligned transaction
          unique case (data_offset)
            2'b00:   data_be = 4'b0011;
            2'b01:   data_be = 4'b0110;
            2'b10:   data_be = 4'b1100;
            2'b11:   data_be = 4'b1000;
            default: data_be = 4'b1111;
          endcase // case (data_offset)
        end else begin // second part of misaligned transaction
          data_be = 4'b0001;
        end
      end

      2'b10,
      2'b11: begin // Writing a byte
        unique case (data_offset)
          2'b00:   data_be = 4'b0001;
          2'b01:   data_be = 4'b0010;
          2'b10:   data_be = 4'b0100;
          2'b11:   data_be = 4'b1000;
          default: data_be = 4'b1111;
        endcase // case (data_offset)
      end

      default:     data_be = 4'b1111;
    endcase // case (lsu_type_i)
  end
  wire cmd_hi = data_addr_i[2];
  assign be = data_be;
  assign strb     = ({8{cmd_hi}}  & {data_be, 4'b0}) 
                          | ({8{!cmd_hi}} & {4'b0, data_be});
endmodule
